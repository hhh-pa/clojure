"""Hot-text order for AOT binaries from Linux workload profiles (#271).

Steps (callgrind profiles of the training workloads, one revision):

  aot_order.py callgrind FILE.cg
      Every runtime function a callgrind run executed (Rust and the
      `rt_*` C ABI), in first-seen order (one workload's function set;
      std's precompiled functions included).
  aot_order.py order --fns DIR --startup FILE --sizes FILE [--previous LD] --out LD
      The layout: the startup set first (in the previous order's
      sequence), then 62 KiB windows filled with functions workloads use
      together (each window seeded with the most used unplaced function,
      then grown by the function whose set of using workloads is most
      similar to the window's), so a program touches few 64 KiB windows
      of the hot section (the kernel faults text in around each touched
      page) and so few pages.

The training set is scripts/bench/hot-text/workloads and aot-workloads:
small generic programs, never the benchmark snippets or their inputs, so
the order reflects the runtime's own hot paths rather than any measured
program. A function the order misses still links, in the cold `.text`."""
import collections
import os
import re
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import hot_text  # noqa: E402

WINDOW = 64 * 1024
CAP = 62 * 1024


def runtime_name(name):
    """Whether `name` is a runtime function the order may place: a Rust
    mangled symbol (v0 or legacy), or one of the runtime's unmangled C-ABI
    entry points (`rt_n_*`, `rt_bc_*`, ...), which every compiled program
    calls (missing them left the ABI scattered over the cold text)."""
    return re.fullmatch(r'(_R|_ZN)[A-Za-z0-9_.$]+|rt_[A-Za-z0-9_]+', name) is not None


def callgrind(path):
    """Runtime functions ([`runtime_name`]) with any cost in a callgrind
    output, in first-seen order (`fn=` ids resolve through their first
    definition)."""
    names, seen = {}, {}
    fn, after_call = None, False
    for line in open(path, errors='replace'):
        line = line.rstrip('\n')
        if line.startswith(('fn=', 'cfn=')):
            spec = line.split('=', 1)[1]
            m = re.match(r'\((\d+)\)\s*(.*)$', spec)
            name = spec
            if m:
                if m.group(2):
                    names[m.group(1)] = m.group(2)
                name = names.get(m.group(1), '?')
            if line.startswith('fn='):
                fn = name
        elif line.startswith('calls='):
            after_call = True
        elif line[:1].isdigit() or line[:1] in '+-*':
            if after_call:
                after_call = False
            elif fn and runtime_name(fn):
                seen.setdefault(fn, None)
    return list(seen)


def load_sizes(path):
    """Symbol sizes by glob from `nm -S --size-sort -t d` output."""
    sizes = {}
    for line in open(path, errors='replace'):
        parts = line.split()
        if len(parts) == 4 and parts[2] in 'tTwW':
            sizes.setdefault(hot_text.glob(parts[3]), int(parts[1]))
    return sizes


def previous_rank(path):
    """Each glob's position in an existing order (`*(.text*.GLOB)` lines)."""
    rank = {}
    if path and os.path.exists(path):
        for line in open(path):
            line = line.strip()
            if line.startswith('*(.text*.') and line.endswith(')'):
                rank.setdefault(line[len('*(.text*.'):-1], len(rank))
    return rank


def layout(progs, startup, size, rank):
    """The function order: startup set first, then the filled windows."""
    count = collections.Counter(f for fns in progs.values() for f in fns)
    first = {}
    for name in sorted(progs):
        for f in sorted(progs[name]):
            first.setdefault(f, len(first))
    ids = {name: i for i, name in enumerate(sorted(progs))}
    users = collections.defaultdict(int)
    for name, fns in progs.items():
        for f in fns:
            users[f] |= 1 << ids[name]
    head = sorted(set(startup), key=lambda f: (rank.get(hot_text.glob(f), 1 << 30), startup.index(f)))
    placed = set(head)
    order = list(head)
    # Candidates are scanned in this fixed order (most used first), so
    # ties resolve the same way on every run: the CI jobs that link the
    # order must all get the same one (set order varies by hash seed).
    by_count = sorted(set(count) - placed, key=lambda f: (-count[f], first[f]))
    unplaced = set(by_count)
    while unplaced:
        seed = next(f for f in by_count if f in unplaced)
        window, used, union = [seed], size(seed), users[seed]
        unplaced.discard(seed)
        while True:
            best, score = None, 0.0
            for f in by_count:
                if f not in unplaced or used + size(f) > CAP:
                    continue
                both = bin(users[f] & union).count('1')
                either = bin(users[f] | union).count('1')
                s = both / either if either else 0.0
                if s > score:
                    best, score = f, s
            if best is None:
                break
            window.append(best)
            used += size(best)
            union |= users[best]
            unplaced.discard(best)
        order += window
    return order


def windows(order, fns, size):
    """How many 64 KiB windows of the hot section `fns` touch."""
    pos, off = {}, 0
    for f in order:
        pos[f] = off
        off += size(f)
    touched = set()
    for f in fns:
        if f in pos:
            touched.update(range(pos[f] // WINDOW, (pos[f] + size(f) - 1) // WINDOW + 1))
    return len(touched)


def write_order(order, path, programs):
    globs, done = [], set()
    for f in order:
        g = hot_text.glob(f)
        if g not in done:
            done.add(g)
            globs.append(g)
    with open(path, 'w') as out:
        out.write('/* Generated by scripts/bench/hot-text/regen-aot.sh from %d workload\n' % programs)
        out.write('   profiles (aot_order.py, #271). Do not edit. */\n')
        out.write('SECTIONS\n{\n  .text.hot :\n  {\n')
        for g in globs:
            # `.text*.` also takes `.text.unlikely.` (`#[cold]` fns).
            out.write('    *(.text*.%s)\n' % g)
        out.write('  }\n}\nINSERT BEFORE .text;\n')
    return len(globs)


def order_main(args):
    opts = dict(zip(args[::2], args[1::2]))
    fns_dir, out = opts['--fns'], opts['--out']
    progs = {}
    for f in sorted(os.listdir(fns_dir)):
        if f.endswith('.txt'):
            fns = [l.strip() for l in open(os.path.join(fns_dir, f)) if l.strip()]
            if fns:
                progs[f[:-4]] = set(fns)
    if not progs:
        sys.exit('aot_order.py: no workload profiles in %s' % fns_dir)
    startup = [l.strip() for l in open(opts['--startup']) if l.strip()]
    sizes = load_sizes(opts['--sizes'])
    median = int(statistics.median(sizes.values())) if sizes else 256
    size = lambda f: sizes.get(hot_text.glob(f)) or median
    order = layout(progs, startup, size, previous_rank(opts.get('--previous')))
    n = write_order(order, out, len(progs))
    per = sorted(windows(order, fns | set(startup), size) for fns in progs.values())
    print('%d workloads, %d functions -> %d globs in %s; windows per workload median %d, p90 %d'
          % (len(progs), len(order), n, out, statistics.median(per), per[9 * len(per) // 10]))


def main():
    cmd, rest = (sys.argv[1], sys.argv[2:]) if len(sys.argv) > 1 else ('', [])
    if cmd == 'callgrind':
        print('\n'.join(callgrind(rest[0])))
    elif cmd == 'order':
        order_main(rest)
    else:
        sys.exit('usage: aot_order.py callgrind FILE.cg | '
                 'order --fns DIR --startup FILE --sizes FILE [--previous LD] --out LD')


if __name__ == '__main__':
    main()
