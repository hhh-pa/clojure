// inscount: minimal Intel Pin instruction-count tool for the
// h2h cpu/mem gate. Counts every executed instruction (all
// threads, all images including the Pin VM itself is excluded by
// construction: only application instructions are instrumented)
// and prints one line:
//
//	Count N
//
// The harness parsers (snip/algoperf ParsePinCount) read the last
// such line. Build out of the kit with:
//
//	make PIN_ROOT=<kit> -C <this dir>
//
// which drops obj-intel64/inscount.so next to this file.
#include <cstdio>
#include "pin.H"

static UINT64 total = 0;
static FILE *out = nullptr;
static KNOB<std::string> KnobOutput(KNOB_MODE_WRITEONCE, "pintool", "o", "inscount.out",
                                    "output file for the Count line");

static VOID docount(UINT32 n) { total += n; }

static VOID Instruction(INS ins, VOID *) {
	INS_InsertCall(ins, IPOINT_BEFORE, (AFUNPTR)docount, IARG_UINT32, 1, IARG_END);
}

static VOID Fini(INT32, VOID *) {
	if (out != nullptr) {
		fprintf(out, "Count %llu\n", (unsigned long long)total);
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
	PIN_AddFiniFunction(Fini, nullptr);
	PIN_StartProgram();
	return 0;
}
