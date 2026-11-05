// inscount: minimal Intel Pin counting tool for the h2h gate.
// Counts every executed instruction and every malloc-family
// requested byte (one run, two signals) and prints:
//
//	Count N
//	Alloc M
//
// The harness parsers (snip/algoperf ParsePinCount/ParsePinAlloc)
// read the last such lines. Build out of the kit with:
//
//	make PIN_ROOT=<kit> -C <this dir>
//
// which drops obj-intel64/inscount.so next to this file.
//
// Allocation semantics: cumulative requested bytes (malloc size,
// calloc nmemb*size, realloc/memalign/aligned_alloc new size,
// posix_memalign size), counted unconditionally at the call entry
// point: an OOM failure kills the run, which fails loudly rather
// than reporting bad data. Matches the LD_PRELOAD counting-shim
// convention: realloc counts its full new size (over-approximates
// shrink/grow deltas), frees are ignored. Only the standard entry
// names are instrumented, never __libc_* aliases, so one app call
// counts once.
#include <cstdio>
#include "pin.H"

static UINT64 total = 0;
static UINT64 allocated = 0;
static FILE *out = nullptr;
static KNOB<std::string> KnobOutput(KNOB_MODE_WRITEONCE, "pintool", "o", "inscount.out",
                                    "output file for the Count/Alloc lines");

static VOID docount(UINT32 n) { __atomic_add_fetch(&total, n, __ATOMIC_RELAXED); }

static VOID onAlloc(ADDRINT size) { __atomic_add_fetch(&allocated, (UINT64)size, __ATOMIC_RELAXED); }

static VOID onAllocProduct(ADDRINT a, ADDRINT b) {
	__atomic_add_fetch(&allocated, (UINT64)a * (UINT64)b, __ATOMIC_RELAXED);
}

static VOID Instruction(INS ins, VOID *) {
	INS_InsertCall(ins, IPOINT_BEFORE, (AFUNPTR)docount, IARG_UINT32, 1, IARG_END);
}

// InstrumentAlloc1 counts one size argument (index arg) of the
// named routine at its entry point: malloc(size),
// realloc(ptr, size), memalign(align, size),
// aligned_alloc(align, size).
static VOID InstrumentAlloc1(IMG img, const char *name, int arg) {
	RTN rtn = RTN_FindByName(img, name);
	if (!RTN_Valid(rtn)) {
		return;
	}
	RTN_Open(rtn);
	RTN_InsertCall(rtn, IPOINT_BEFORE, (AFUNPTR)onAlloc, IARG_FUNCARG_ENTRYPOINT_VALUE, arg, IARG_END);
	RTN_Close(rtn);
}

static VOID Image(IMG img, VOID *) {
	InstrumentAlloc1(img, "malloc", 0);
	InstrumentAlloc1(img, "realloc", 1);
	InstrumentAlloc1(img, "memalign", 1);
	InstrumentAlloc1(img, "aligned_alloc", 1);
	RTN rtn = RTN_FindByName(img, "calloc");
	if (RTN_Valid(rtn)) {
		RTN_Open(rtn);
		RTN_InsertCall(rtn, IPOINT_BEFORE, (AFUNPTR)onAllocProduct, IARG_FUNCARG_ENTRYPOINT_VALUE, 0,
		               IARG_FUNCARG_ENTRYPOINT_VALUE, 1, IARG_END);
		RTN_Close(rtn);
	}
	// posix_memalign(&ptr, align, size): size is argument 2.
	rtn = RTN_FindByName(img, "posix_memalign");
	if (RTN_Valid(rtn)) {
		RTN_Open(rtn);
		RTN_InsertCall(rtn, IPOINT_BEFORE, (AFUNPTR)onAlloc, IARG_FUNCARG_ENTRYPOINT_VALUE, 2, IARG_END);
		RTN_Close(rtn);
	}
}

static VOID Fini(INT32, VOID *) {
	if (out != nullptr) {
		fprintf(out, "Count %llu\n", (unsigned long long)total);
		fprintf(out, "Alloc %llu\n", (unsigned long long)allocated);
		fclose(out);
	}
}

int main(int argc, char *argv[]) {
	PIN_InitSymbols();
	if (PIN_Init(argc, argv)) {
		return 1;
	}
	out = fopen(KnobOutput.Value().c_str(), "w");
	if (out == nullptr) {
		return 1;
	}
	INS_AddInstrumentFunction(Instruction, nullptr);
	IMG_AddInstrumentFunction(Image, nullptr);
	PIN_AddFiniFunction(Fini, nullptr);
	PIN_StartProgram();
	return 0;
}
