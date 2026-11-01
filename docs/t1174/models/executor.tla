(* Copyright (c) Rich Hickey. All rights reserved. *)
(* The use and distribution terms for this software are covered by the *)
(* Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php) *)
(* which can be found in the file epl-v10.html at the root of this distribution. *)
(* By using this software in any fashion, you are agreeing to be bound by *)
(* the terms of this license. *)
(* You must not remove this notice, or any other, from this software. *)

---- MODULE executor ----
(***************************************************************************)
(* #188: single-thread executor cell (fence 3c / gap-96).                  *)
(*                                                                         *)
(* Models ONE executor pool plus its submit/shutdown/worker protocol,     *)
(* abstracting rust/crates/clj-value/src/executor.rs (Executor::submit,   *)
(* shutdown, is_shutdown, the worker drain loop) and the gap-96           *)
(* shutdown-agents contract (installed pools shut together; queued jobs  *)
(* still run, later submits reject). Mirrors the loom model in           *)
(* rust/crates/clj-value/tests/loom_executor.rs at the                    *)
(* submit-vs-shutdown race level (the loom mirror owns the schedule      *)
(* exploration -- a racing submit either runs or rejects, never both    *)
(* nor neither; this spec owns FIFO order, drain-then-reject, and       *)
(* shutdown idempotence).                                                 *)
(*                                                                         *)
(* TLC FILE-NAME NOTE: TLA+ module names cannot contain hyphens, so     *)
(* this file must stay named executor.tla (basename == module name).     *)
(*                                                                         *)
(* Run (from docs/t1174/models/):                                          *)
(*   java -cp tla2tools.jar tlc2.TLC -workers auto -cleanup \               *)
(*     -config executor.cfg executor.tla                                   *)
(*                                                                         *)
(* SCOPE (what is modeled):                                                *)
(*   - submit: non-blocking, unbounded queue, FIFO; false once shut      *)
(*     (SubmitRejectsAfterShutdown, FifoOrder).                           *)
(*   - worker: one thread drains head-first, including after shutdown    *)
(*     (DrainAfterShutdown); queued jobs all eventually run (Liveness).  *)
(*   - shutdown: idempotent latch (ShutLatch, ShutdownIdempotent);      *)
(*     rejected jobs never run (RejectedNeverRuns).                      *)
(*                                                                         *)
(* DELIBERATE ABSTRACTIONS (gaps vs the Rust code):                        *)
(*   G1 mpsc mechanics: the unbounded channel's internal send/recv       *)
(*      races are NOT modeled -- submit-then-run is an atomic enqueue    *)
(*      plus an atomic dequeue (std owns the queue; the loom mirror      *)
(*      keeps the same boundary with a Mutex+VecDeque+Condvar stand-in). *)
(*   G2 job bodies: jobs are opaque closures (agent heads arrive        *)
(*      wrapped); bodies run atomically here -- no shared state crosses *)
(*      the pool boundary in the model.                                  *)
(*   G3 thread identity: the pool-N-thread-1 naming and the             *)
(*      process-wide pool counter are NOT modeled (pinned by the        *)
(*      in-module tests). Worker detach (never joined) is NOT modeled  *)
(*      -- Run is always enabled on a nonempty queue, which is the     *)
(*      daemon-equivalent liveness assumption.                         *)
(*   G4 install/shutdown-agents fan-out: which installed fields shut    *)
(*      together lives in clj-eval (gap-96); here one pool shuts once. *)
(***************************************************************************)

EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Submitters, \* Concurrent submitter threads, e.g. {s1, s2}
    J           \* Jobs per submitter

\* A job is <<submitter, seq>>.
JobSet == Submitters \X (1 .. J)

VARIABLES
    queue, \* Queued jobs, head first
    shut,  \* Shutdown latch (sender slot is None)
    shutAt,\* Len(acc) when Shutdown fired, else -1
    sent,  \* [Submitters -> 0..J] jobs submitted (accepted+rejected)
    acc,   \* Accepted jobs, in accept order
    log,   \* Run jobs, in run order
    rej    \* Rejected jobs (post-shutdown submits)

vars == <<queue, shut, shutAt, sent, acc, log, rej>>

(***************************************************************************)
(* Helpers.                                                                *)
(***************************************************************************)

QueueSet == { queue[i] : i \in DOMAIN queue }
AccSet == { acc[i] : i \in DOMAIN acc }
LogSet == { log[i] : i \in DOMAIN log }

RECURSIVE IsSubseq(_, _)
IsSubseq(a, b) ==
    IF a = <<>> THEN TRUE
    ELSE IF b = <<>> THEN FALSE
    ELSE IF Head(a) = Head(b) THEN IsSubseq(Tail(a), Tail(b))
    ELSE IsSubseq(a, Tail(b))

NoDupSeq(s) == Cardinality({ s[i] : i \in DOMAIN s }) = Len(s)

Disjoint(A, B) == A \cap B = {}

(***************************************************************************)
(* Type invariant.                                                         *)
(***************************************************************************)

TypeOK ==
    /\ queue \in Seq(JobSet)
    /\ shut \in BOOLEAN
    /\ shutAt = -1 \/ shutAt \in Nat
    /\ sent \in [Submitters -> 0 .. J]
    /\ acc \in Seq(JobSet)
    /\ log \in Seq(JobSet)
    /\ rej \in SUBSET JobSet

(***************************************************************************)
(* Initial state.                                                          *)
(***************************************************************************)

Init ==
    /\ queue = <<>>
    /\ shut = FALSE
    /\ shutAt = -1
    /\ sent = [s \in Submitters |-> 0]
    /\ acc = <<>>
    /\ log = <<>>
    /\ rej = {}

(***************************************************************************)
(* Submit(s): Executor::submit -- non-blocking; the queue is unbounded   *)
(* (no guard but the script), so post-shutdown submits reject instead of *)
(* parking. The under-slot-lock check-then-send is one atomic step (G1). *)
(***************************************************************************)

Submit(s) ==
    /\ sent[s] < J
    /\ LET j == <<s, sent[s] + 1>> IN
        /\ sent' = [sent EXCEPT ![s] = sent[s] + 1]
        /\ IF shut
           THEN (* Slot is None: reject (the job never runs). *)
                /\ rej' = rej \union {j}
                /\ UNCHANGED <<queue, acc>>
           ELSE (* Slot is Some: the unbounded send always succeeds. *)
                /\ queue' = Append(queue, j)
                /\ acc' = Append(acc, j)
                /\ UNCHANGED rej
    /\ UNCHANGED <<shut, shutAt, log>>

(***************************************************************************)
(* Run: the worker loop head -- pop the head job and run it (G2: the body *)
(* is atomic). Enabled past shutdown: the worker drains the queue, then *)
(* idles (the recv loop ends only when the drained queue meets the     *)
(* dropped sender; idling here is that exit).                           *)
(***************************************************************************)

Run ==
    /\ queue # <<>>
    /\ log' = Append(log, Head(queue))
    /\ queue' = Tail(queue)
    /\ UNCHANGED <<shut, shutAt, sent, acc, rej>>

(***************************************************************************)
(* Shutdown: Executor::shutdown -- idempotent latch (the second call is a *)
(* no-op by construction, so one guarded action covers racing shutdowns; *)
(* G4: one pool, installed-fields fan-out lives in clj-eval).           *)
(***************************************************************************)

Shutdown ==
    /\ ~shut
    /\ shut' = TRUE
    /\ shutAt' = Len(acc)
    /\ UNCHANGED <<queue, sent, acc, log, rej>>

(***************************************************************************)
(* Terminating: every script is submitted and the queue drained; stutter *)
(* (keeps TLC deadlock-free so a deadlock report always means a modeling *)
(* bug -- submission never blocks and the worker never parks on work).  *)
(***************************************************************************)

AllSent == \A s \in Submitters : sent[s] = J

Terminating ==
    /\ AllSent
    /\ queue = <<>>
    /\ UNCHANGED vars

Next ==
    \/ \E s \in Submitters : Submit(s)
    \/ Run
    \/ Shutdown
    \/ Terminating

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Liveness: every accepted job eventually runs (weak fairness on the    *)
(* worker and every submitter -- the daemon-equivalent assumption, G3). *)
(* Checked as a temporal PROPERTY on FairSpec.                          *)
(***************************************************************************)

Fairness == (\A s \in Submitters : WF_vars(Submit(s))) /\ WF_vars(Run)
FairSpec == Spec /\ Fairness
Liveness == <>(AllSent /\ queue = <<>>)

(***************************************************************************)
(* INVARIANT MAP -- each Safety conjunct -> the Rust path implementing it. *)
(*                                                                         *)
(* FifoOrder: log is a subsequence of acc (single worker, head-first).   *)
(*   -> executor.rs: the mpsc queue preserves send order; the worker's   *)
(*   recv loop runs head-first; loom FIFO pin.                           *)
(* EachOnce: log/queue/acc hold no duplicates.                           *)
(*   -> ExecutorJob moves BY VALUE (sent once, received once, run once).*)
(* HeldAccounted: every accepted job sits in exactly one of log/queue.  *)
(*   -> the ownership discipline across submit (enqueue) and the worker  *)
(*   (dequeue-then-run).                                                 *)
(* ShutLatch / ShutAtBound: the latch is monotonic and shutAt pins the  *)
(*   pre/post-shutdown accept boundary. -> executor.rs: shutdown takes  *)
(*   the sender slot once; submit reads it under the same lock.         *)
(* ShutAtStable: no accepts after the latch (Len(acc) frozen).           *)
(*   -> executor.rs: post-shutdown submit reads None and rejects.       *)
(* SubmitRejectsAfterShutdown: every rejection is post-latch.            *)
(*   -> executor.rs: submit returns false only on the None slot (G1: the *)
(*   dead-worker send-failure arm is unreachable -- the worker outlives *)
(*   the sender -- and stays unmodeled).                                 *)
(* RejectedNeverRuns: rejected jobs never run nor queue.                 *)
(*   -> executor.rs: submit drops the job on the false return. Loom     *)
(*   race pin (submit-vs-shutdown: runs XOR rejects).                    *)
(* DrainAfterShutdown: the worker needs no latch check (Run has no shut *)
(*   guard by construction); the observable pin is ShutAtStable +       *)
(*   HeldAccounted + Liveness -- queued jobs still run, later submits   *)
(*   reject (gap-96 shutdown-agents contract).                           *)
(***************************************************************************)

FifoOrder == IsSubseq(log, acc)
EachOnce == NoDupSeq(log) /\ NoDupSeq(queue) /\ NoDupSeq(acc)
HeldAccounted ==
    /\ AccSet \subseteq LogSet \union QueueSet
    /\ LogSet \union QueueSet \subseteq AccSet
    /\ Disjoint(LogSet, QueueSet)
ShutLatch == shut <=> shutAt >= 0
ShutAtBound == shut => shutAt <= Len(acc)
ShutAtStable == shut => Len(acc) = shutAt
SubmitRejectsAfterShutdown == rej # {} => shut
RejectedNeverRuns == Disjoint(rej, LogSet \union QueueSet \union AccSet)

Safety ==
    /\ FifoOrder
    /\ EachOnce
    /\ HeldAccounted
    /\ ShutLatch
    /\ ShutAtBound
    /\ ShutAtStable
    /\ SubmitRejectsAfterShutdown
    /\ RejectedNeverRuns

THEOREM FairSpec => []TypeOK
THEOREM FairSpec => []Safety
THEOREM FairSpec => Liveness

=============================================================================
