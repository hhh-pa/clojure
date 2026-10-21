(* Copyright (c) Rich Hickey. All rights reserved. *)
(* The use and distribution terms for this software are covered by the *)
(* Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php) *)
(* which can be found in the file epl-v10.html at the root of this distribution. *)
(* By using this software in any fashion, you are agreeing to be bound by *)
(* the terms of this license. *)
(* You must not remove this notice, or any other, from this software. *)

---- MODULE agent_queue ----
(***************************************************************************)
(* S9D: agent send/send-off queue discipline.                               *)
(*                                                                         *)
(* Models ONE agent cell plus its submit/complete/restart/shutdown         *)
(* protocol, abstracting the spec in docs/t1174/concurrency-contract.md    *)
(* (Agents), the oracle pins in docs/t1174/s6-pins.md (P5), the cell in    *)
(* rust/crates/clj-value/src/agent.rs, and the submit/dispatch/complete/   *)
(* restart/shutdown paths in rust/crates/clj-eval/src/concurrency.rs      *)
(* (send_action, pools_dispatch, dispatch_sendoff, pump_agent,             *)
(* execute_head, fail_head, release_outbox, native_restart_agent,          *)
(* AgentSystem::shutdown). Mirrors the loom model in                      *)
(* rust/crates/clj-value/tests/loom_agent.rs + loom_common/mod.rs at the  *)
(* queue-discipline level (the loom mirror covers blind-publish lost      *)
(* updates; this spec covers ordering, rejection, restart races, and      *)
(* shutdown -- including the shutdown/reject paths the loom mirror        *)
(* explicitly does NOT model).                                             *)
(*                                                                         *)
(* TLC FILE-NAME NOTE: TLA+ module names cannot contain hyphens, so     *)
(* this file must stay named agent_queue.tla (basename == module name).   *)
(*                                                                         *)
(* Run (from docs/t1174/models/):                                          *)
(*   java -cp tla2tools.jar tlc2.TLC -workers auto -cleanup \               *)
(*     -config agent_queue.cfg agent_queue.tla                             *)
(*                                                                         *)
(* SCOPE (what is modeled):                                                *)
(*   - per-agent FIFO: direct sends queue behind the runner and drain in   *)
(*     accept order (FifoOrder); each action runs at most once (EachOnce) *)
(*   - send vs send-off: every action records its pool, fixed per      *)
(*     sender per behavior (Init chooses every assignment, so one run  *)
(*     exercises both pools); NEITHER pool blocks the submitter        *)
(*     (Liveness) -- the unbounded mpsc queue                          *)
(*     and thread-per-action dispatch_sendoff both return at once.      *)
(*     (A per-send pool choice would multiply the space 2^#sends while  *)
(*     no property reads the pool field.)                               *)
(*   - failed-agent rejection: sends on a failed agent are rejected and   *)
(*     never run (FailedReject); nested sends bypass the check but never  *)
(*     start a runner on a failed agent (ParkedClean, NestedNeverRejected)*)
(*   - restart races: two-phase validate-then-publish; the under-lock     *)
(*     re-check (M3 fix) lets at most one racer publish per failure       *)
(*     episode (AtMostOneOk, OkImpliesClean); :clear-actions drops the    *)
(*     held queue silently (HeldAccounted via `gone`).                    *)
(*   - shutdown: the latch (shut) wedges the queue -- the running flag    *)
(*     stays TRUE while heads drop via reject_head (SingleRunner,         *)
(*     DroppedAfterShutdown); in-flight heads finish; post-shutdown      *)
(*     accepts never run (PostShutdownNeverRuns).                         *)
(*                                                                         *)
(* DELIBERATE ABSTRACTIONS (gaps vs the Rust code):                        *)
(*   G1 state values: actions publish state (execute_head read/publish);  *)
(*      values, validators, watches, and lost-update races are NOT        *)
(*      modeled -- the loom mirror (loom_common run_one) owns those.      *)
(*   G2 nested-send outbox buffering: sends issued INSIDE an action fn    *)
(*      buffer in agent_outbox and release on success / discard on        *)
(*      failure (EvalAction::run, release_outbox). Here nested sends      *)
(*      arrive directly at enqueue_nested; the release/discard step is    *)
(*      not modeled.                                                      *)
(*   G3 await/await-for barriers: barriers are plain sends over the send *)
(*      pool plus a latch (send_barriers, await_latches); the latch, the *)
(*      failure-while-waiting throw, and the post-shutdown disconnect    *)
(*      (contract DIVERGENCE) are NOT modeled. FIFO + log order give the *)
(*      barrier its "sees prior sends" argument informally.              *)
(*   G4 error-handler invocation: reject_head notifies the handler with  *)
(*      RejectedExecutionException, fail_head runs it ignoring failures; *)
(*      handler effects (including handler-originated sends) are NOT     *)
(*      modeled -- `dropped` records only that the handler was told.     *)
(*   G5 dynamic mode/handler setters: set-error-mode! / set-error-handler!*)
(*      can flip the cell at runtime; here `mode` is fixed per behavior  *)
(*      (both values explored via Init nondeterminism).                   *)
(*   G6 pool internals: worker counts, mpsc drain-vs-drop races for      *)
(*      pool-queued heads, send-off thread reaping, and the blocking     *)
(*      join (incl. join_all self-skip) are NOT modeled; dispatch is an  *)
(*      atomic hand-off, shutdown an atomic latch.                        *)
(*   G7 future-cancel: "cancel" here means :clear-actions queue drops (+  *)
(*      barrier drops informally); future-cancel belongs to a future     *)
(*      spec, not this agent-queue spec.                                 *)
(***************************************************************************)

EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Senders,    \* Concurrent submitter threads, e.g. {s1, s2}
    Restarters, \* Threads racing restart-agent, e.g. {r1, r2}
    K,          \* Direct sends per sender
    NESTED      \* Nested (outbox-release) sends per sender

\* An action is <<submitter, seq, kind, pool>>. kind = "send" for direct
\* sends (send/send-off/send-via -- send-via routes to send-off, a pinned
\* divergence) and "nested" for outbox releases. pool is the QueuedAction
\* pool that would run it. NoAct marks "no head under execution".
NoAct == <<"none", 0, "none", "none">>

VARIABLES
    queue,      \* Waiting actions, head first (the running head is popped)
    busy,       \* AgentInner.running
    cur,        \* Dequeued head under execution, or NoAct
    failed,     \* Latched failure present (AgentInner.error.is_some())
    mode,       \* "fail" | "continue" (ErrorMode)
    poolOf,     \* [Senders -> {"send", "sendoff"}]: each sender's pool
    shut,       \* AgentSystem shutdown latch
    shutAt,     \* Len(accepted) when Shutdown fired, else -1
    sent,       \* [Senders -> 0..K] direct sends consumed (accepted+rejected)
    nestedLeft, \* [Senders -> 0..NESTED] nested sends remaining
    log,        \* Successful completions, in run order
    accepted,   \* Accepted sends (direct + nested), in accept order
    rej,        \* Sends rejected on a failed agent
    dropped,    \* Heads dropped via reject_head (handler notified)
    gone,       \* Failed heads + :clear-actions drops (silent)
    phase,      \* [Restarters -> {"idle","validating"}] (pre-publish window)
    result      \* [Restarters -> {"none","ok","rejected"}] (last publish)

vars == <<queue, busy, cur, failed, mode, poolOf, shut, shutAt, sent,
          nestedLeft, log, accepted, rej, dropped, gone, phase, result>>

ActionSet ==
    UNION { { <<t, n, "send", poolOf[t]>> : n \in 1 .. K } : t \in Senders }
      \union
    UNION { { <<t, n, "nested", poolOf[t]>> : n \in 1 .. NESTED } : t \in Senders }

(***************************************************************************)
(* Helpers.                                                                *)
(***************************************************************************)

QueueSet == { queue[i] : i \in DOMAIN queue }
LogSet == { log[i] : i \in DOMAIN log }
AcceptedSet == { accepted[i] : i \in DOMAIN accepted }
CurSet == IF cur = NoAct THEN {} ELSE {cur}

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
    /\ queue \in Seq(ActionSet)
    /\ busy \in BOOLEAN
    /\ cur \in ActionSet \union {NoAct}
    /\ failed \in BOOLEAN
    /\ mode \in {"fail", "continue"}
    /\ poolOf \in [Senders -> {"send", "sendoff"}]
    /\ shut \in BOOLEAN
    /\ shutAt = -1 \/ shutAt \in Nat
    /\ sent \in [Senders -> 0 .. K]
    /\ nestedLeft \in [Senders -> 0 .. NESTED]
    /\ log \in Seq(ActionSet)
    /\ accepted \in Seq(ActionSet)
    /\ rej \in SUBSET ActionSet
    /\ dropped \in SUBSET ActionSet
    /\ gone \in SUBSET ActionSet
    /\ phase \in [Restarters -> {"idle", "validating"}]
    /\ result \in [Restarters -> {"none", "ok", "rejected"}]

(***************************************************************************)
(* Initial state. `mode' and `poolOf' are chosen nondeterministically so   *)
(* one TLC run covers both :fail and :continue and every pool assignment  *)
(* (G5: fixed per behavior).                                               *)
(***************************************************************************)

Init ==
    /\ queue = <<>>
    /\ busy = FALSE
    /\ cur = NoAct
    /\ failed = FALSE
    /\ mode \in {"fail", "continue"}
    /\ poolOf \in [Senders -> {"send", "sendoff"}]
    /\ shut = FALSE
    /\ shutAt = -1
    /\ sent = [t \in Senders |-> 0]
    /\ nestedLeft = [t \in Senders |-> NESTED]
    /\ log = <<>>
    /\ accepted = <<>>
    /\ rej = {}
    /\ dropped = {}
    /\ gone = {}
    /\ phase = [r \in Restarters |-> "idle"]
    /\ result = [r \in Restarters |-> "none"]

(***************************************************************************)
(* Send(t): send_action for one direct send. No failed pre-check (the     *)
(* real code relies on enqueue's under-lock check); enqueue then          *)
(* dispatch-or-drop. The failed check beats shutdown: a failed agent      *)
(* rejects even after the latch.                                          *)
(***************************************************************************)

Send(t) ==
    /\ sent[t] < K
    /\ LET a == <<t, sent[t] + 1, "send", poolOf[t]>> IN
        /\ sent' = [sent EXCEPT ![t] = sent[t] + 1]
        /\ IF failed
           THEN (* Agent::enqueue rejects under the cell lock. *)
                /\ rej' = rej \union {a}
                /\ UNCHANGED <<queue, busy, cur, accepted, dropped>>
           ELSE /\ accepted' = Append(accepted, a)
                /\ rej' = rej
                /\ IF busy
                   THEN (* Runner owns the chain; completion picks it up. *)
                        /\ queue' = Append(queue, a)
                        /\ UNCHANGED <<busy, cur, dropped>>
                   ELSE IF shut
                        THEN (* Idle: the head pops at enqueue, the pools *)
                             (* are gone, reject_head drops it; running  *)
                             (* wedges TRUE (the contract's "wedge").    *)
                             /\ queue' = queue
                             /\ busy' = TRUE
                             /\ cur' = NoAct
                             /\ dropped' = dropped \union {a}
                        ELSE (* Idle: the head pops at enqueue and        *)
                             (* dispatches to its pool.                  *)
                             /\ queue' = queue
                             /\ busy' = TRUE
                             /\ cur' = a
                             /\ dropped' = dropped
    /\ UNCHANGED <<failed, mode, poolOf, shut, shutAt, nestedLeft, log, gone,
                   phase, result>>

(***************************************************************************)
(* NestedSend(t): release_outbox delivering one buffered nested send to   *)
(* Agent::enqueue_nested: unchecked (bypasses the failed check, like the *)
(* JVM's releasePendingSends), and the runner starts only when idle AND  *)
(* clean -- onto a failed agent the send queues but never executes.      *)
(* (G2: the buffer/release/discard step itself is not modeled.)           *)
(***************************************************************************)

NestedSend(t) ==
    /\ nestedLeft[t] > 0
    /\ LET b == <<t, nestedLeft[t], "nested", poolOf[t]>> IN
        /\ nestedLeft' = [nestedLeft EXCEPT ![t] = @ - 1]
        /\ accepted' = Append(accepted, b)
        /\ rej' = rej
        /\ IF failed
           THEN (* Queues, never starts; waits for a resume. *)
                /\ queue' = Append(queue, b)
                /\ UNCHANGED <<busy, cur, dropped>>
           ELSE IF busy
                THEN /\ queue' = Append(queue, b)
                     /\ UNCHANGED <<busy, cur, dropped>>
                ELSE IF shut
                     THEN /\ queue' = queue
                          /\ busy' = TRUE
                          /\ cur' = NoAct
                          /\ dropped' = dropped \union {b}
                     ELSE /\ queue' = queue
                          /\ busy' = TRUE
                          /\ cur' = b
                          /\ dropped' = dropped
    /\ UNCHANGED <<failed, mode, poolOf, shut, shutAt, sent, log, gone,
                   phase, result>>

(***************************************************************************)
(* Complete: execute_head succeeded (publish + watches + outbox release), *)
(* then Agent::complete_ok hands the next head over -- except after       *)
(* shutdown, where pump_agent drops the successor via reject_head while   *)
(* the in-flight head still finishes.                                     *)
(***************************************************************************)

Complete ==
    /\ busy
    /\ cur # NoAct
    /\ ~failed
    /\ log' = Append(log, cur)
    /\ IF shut
       THEN (* In-flight head finishes; the successor drops. *)
            /\ IF queue = <<>>
               THEN /\ cur' = NoAct
                    /\ busy' = FALSE
                    /\ UNCHANGED <<queue, dropped>>
               ELSE /\ dropped' = dropped \union {Head(queue)}
                    /\ queue' = Tail(queue)
                    /\ cur' = NoAct
                    /\ busy' = TRUE
       ELSE (* complete_ok hands the next head to its pool. *)
            /\ IF queue = <<>>
               THEN /\ cur' = NoAct
                    /\ busy' = FALSE
                    /\ UNCHANGED queue
               ELSE /\ cur' = Head(queue)
                    /\ queue' = Tail(queue)
                    /\ UNCHANGED busy
            /\ UNCHANGED dropped
    /\ UNCHANGED <<failed, mode, poolOf, shut, shutAt, sent, nestedLeft,
                   accepted, rej, gone, phase, result>>

(***************************************************************************)
(* FailHead: execute_head failed (action fn, validator veto, or throwing  *)
(* watch -- the three are one nondeterministic choice here, G1).          *)
(* fail_head runs the handler (G4), then :continue discards the head and *)
(* proceeds via complete_ok, while :fail latches (complete_failed),      *)
(* idles the runner, and holds the queue. A :fail latch opens a new      *)
(* restart episode (prior per-restarter outcomes are superseded).         *)
(***************************************************************************)

FailHead ==
    /\ busy
    /\ cur # NoAct
    /\ ~failed
    /\ gone' = gone \union {cur}
    /\ IF mode = "continue"
       THEN /\ UNCHANGED <<failed, result>>
            /\ IF shut
               THEN (* complete_ok hands next; pump_agent rejects it. *)
                    /\ UNCHANGED log
                    /\ IF queue = <<>>
                       THEN /\ cur' = NoAct
                            /\ busy' = FALSE
                            /\ UNCHANGED <<queue, dropped>>
                       ELSE /\ dropped' = dropped \union {Head(queue)}
                            /\ queue' = Tail(queue)
                            /\ cur' = NoAct
                            /\ busy' = TRUE
               ELSE (* Discard the head, hand the next to its pool. *)
                    /\ UNCHANGED <<log, dropped>>
                    /\ IF queue = <<>>
                       THEN /\ cur' = NoAct
                            /\ busy' = FALSE
                            /\ UNCHANGED queue
                       ELSE /\ cur' = Head(queue)
                            /\ queue' = Tail(queue)
                            /\ UNCHANGED busy
       ELSE (* :fail latches, idles, holds the queue; new episode. *)
            /\ failed' = TRUE
            /\ result' = [r \in Restarters |-> "none"]
            /\ cur' = NoAct
            /\ busy' = FALSE
            /\ UNCHANGED <<queue, log, dropped>>
    /\ UNCHANGED <<mode, poolOf, shut, shutAt, sent, nestedLeft,
                   accepted, rej, phase>>

(***************************************************************************)
(* BeginRestart(r): native_restart_agent pre-check passed (failed) and the *)
(* restarter enters the options/validation window OUTSIDE the cell lock   *)
(* (arbitrary code must not run under the non-reentrant mutex).           *)
(***************************************************************************)

BeginRestart(r) ==
    /\ phase[r] = "idle"
    /\ failed
    /\ phase' = [phase EXCEPT ![r] = "validating"]
    /\ UNCHANGED <<queue, busy, cur, failed, mode, poolOf, shut, shutAt, sent,
                   nestedLeft, log, accepted, rej, dropped, gone, result>>

(***************************************************************************)
(* PublishRestart(r): Agent::restart_publish under the cell lock. The     *)
(* failed re-check (M3 fix) serializes racers: exactly the first publish *)
(* per episode sees the latched error; losers report "does not need a    *)
(* restart". A resume pops the held head (and dispatches it -- or drops  *)
(* it via reject_head after shutdown, the restart still succeeding);     *)
(* :clear-actions drops the held queue SILENTLY (no handler notify).     *)
(* (G1: the restart state value and validator verdict are not modeled;   *)
(* a veto never reaches the publish, so every publish here succeeds.)    *)
(***************************************************************************)

PublishRestart(r) ==
    /\ phase[r] = "validating"
    /\ phase' = [phase EXCEPT ![r] = "idle"]
    /\ \E clear \in BOOLEAN :
        IF failed
        THEN (* Under-lock re-check passes: publish. *)
             /\ failed' = FALSE
             /\ result' = [result EXCEPT ![r] = "ok"]
             /\ IF clear
                THEN (* :clear-actions drops the held queue, silently. *)
                     /\ gone' = gone \union QueueSet
                     /\ queue' = <<>>
                     /\ busy' = FALSE
                     /\ cur' = NoAct
                     /\ UNCHANGED dropped
                ELSE IF queue = <<>>
                     THEN /\ busy' = FALSE
                          /\ cur' = NoAct
                          /\ UNCHANGED <<queue, gone, dropped>>
                     ELSE IF shut
                          THEN (* Resumed head drops via reject_head. *)
                               /\ dropped' = dropped \union {Head(queue)}
                               /\ queue' = Tail(queue)
                               /\ busy' = TRUE
                               /\ cur' = NoAct
                               /\ UNCHANGED gone
                          ELSE (* Resume the head on its pool. *)
                               /\ cur' = Head(queue)
                               /\ queue' = Tail(queue)
                               /\ busy' = TRUE
                               /\ UNCHANGED <<gone, dropped>>
        ELSE (* Loser: the error already cleared. *)
             /\ result' = [result EXCEPT ![r] = "rejected"]
             /\ UNCHANGED <<failed, queue, busy, cur, gone, dropped>>
    /\ UNCHANGED <<mode, poolOf, shut, shutAt, sent, nestedLeft,
                   log, accepted, rej>>

(***************************************************************************)
(* Shutdown: AgentSystem::shutdown latches (G6: the blocking join of pool *)
(* workers, send-off threads, and future threads is not modeled).        *)
(***************************************************************************)

Shutdown ==
    /\ ~shut
    /\ shut' = TRUE
    /\ shutAt' = Len(accepted)
    /\ UNCHANGED <<queue, busy, cur, failed, mode, poolOf, sent, nestedLeft,
                   log, accepted, rej, dropped, gone, phase, result>>

(***************************************************************************)
(* Terminating: all submitters are done; stutter (keeps TLC deadlock-free *)
(* so a deadlock report always means a modeling bug, never the end of a  *)
(* finite submission script).                                             *)
(***************************************************************************)

AllSendsDone == \A t \in Senders : sent[t] = K /\ nestedLeft[t] = 0

Terminating ==
    /\ AllSendsDone
    /\ UNCHANGED vars

Next ==
    \/ \E t \in Senders : Send(t) \/ NestedSend(t)
    \/ Complete
    \/ FailHead
    \/ \E r \in Restarters : BeginRestart(r) \/ PublishRestart(r)
    \/ Shutdown
    \/ Terminating

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Non-blocking submission: every submitter eventually consumes its whole *)
(* script -- accepted, rejected, or dropped -- REGARDLESS of runner,     *)
(* failure, or shutdown state. This is the checkable form of "send-off   *)
(* (and send) never block the caller": neither Send nor NestedSend has   *)
(* any guard beyond its own remaining script. Checked as a temporal      *)
(* PROPERTY on FairSpec (weak fairness per submitter).                   *)
(***************************************************************************)

Fairness == \A t \in Senders : WF_vars(Send(t) \/ NestedSend(t))
FairSpec == Spec /\ Fairness
Liveness == \A t \in Senders : <>(sent[t] = K /\ nestedLeft[t] = 0)

(***************************************************************************)
(* INVARIANT MAP -- each Safety conjunct -> the Rust path implementing it. *)
(*                                                                         *)
(* SingleRunner: (cur # NoAct => busy) /\ ((busy /\ cur = NoAct) => shut) *)
(*   At most one runner per agent; a runner-less busy cell is wedged by   *)
(*   shutdown. -> agent.rs: enqueue sets running together with the pop;   *)
(*   complete_ok hands over or idles; complete_failed idles.             *)
(*   concurrency.rs: send_action pools-None branch, pump_agent            *)
(*   is_shutdown branch, pools_dispatch None branch, and the             *)
(*   native_restart_agent resume-None branch all call reject_head, which *)
(*   drops the head WITHOUT clearing running (the wedge).                *)
(* ParkedClean: failed => ~busy /\ cur = NoAct                            *)
(*   A failed agent runs nothing. -> agent.rs: complete_failed latches   *)
(*   + idles; enqueue_nested never starts when error.is_some().          *)
(* IdleCleanEmpty: (~busy /\ ~failed) => queue = <<>>                     *)
(*   An idle clean agent holds no queue. -> agent.rs: enqueue starts the *)
(*   runner when idle; complete_ok idles only on drain; restart_publish *)
(*   clear/empty branches idle.                                          *)
(* ShutLatch / ShutAtBound: the latch is monotonic and shutAt pins the   *)
(*   pre/post-shutdown accept boundary. -> concurrency.rs:          *)
(*   AgentSystem::shutdown latches once; pools_for_send is None after.   *)
(* FailedReject: rejected sends never run nor queue.                     *)
(*   -> agent.rs: enqueue Err branch returns the latched error;          *)
(*   concurrency.rs: send_action maps it to agent_failed (RuntimeEx.);   *)
(*   contract: send/await/await-for throw until restart-agent (P5).      *)
(* NestedNeverRejected: nested sends are never rejected.                 *)
(*   -> agent.rs: enqueue_nested has no failed check (the JVM's          *)
(*   releasePendingSends bypass); release_outbox always enqueues.        *)
(* FifoOrder: log is a subsequence of accepted (per-agent FIFO).         *)
(*   -> agent.rs: VecDeque push_back/pop_front in enqueue, complete_ok, *)
(*   restart_publish; the single-runner handoff chain preserves order.  *)
(* EachOnce: log/queue/accepted hold no duplicates.                      *)
(*   -> QueuedAction moves BY VALUE (runs exactly once); loom P3a.       *)
(* HeldAccounted: every accepted action sits in exactly one of           *)
(*   log/queue/cur/dropped/gone. -> the ownership discipline across     *)
(*   enqueue, complete_ok, complete_failed, restart_publish (resume     *)
(*   pops / :clear-actions drains), and reject_head.                      *)
(* DroppedAfterShutdown: dropped = {} \/ shut.                            *)
(*   -> concurrency.rs: reject_head is reached only post-latch          *)
(*   (send_action pools-None, pump_agent is_shutdown, pools_dispatch    *)
(*   None, restart resume-None).                                         *)
(* PostShutdownNeverRuns: accepts after shutAt never enter the log.      *)
(*   In-flight (pre-shutdown) heads still finish -- the wedge only      *)
(*   stops what was accepted after the latch. -> the running=TRUE wedge  *)
(*   + reject_head drops; contract "the queue wedges".                   *)
(* AtMostOneOk / OkImpliesClean: per failure episode, at most one       *)
(*   restarter publishes, and an "ok" means the cell is clean.           *)
(*   -> agent.rs: restart_publish re-checks error UNDER THE LOCK (M3    *)
(*   fix); concurrency.rs: native_restart_agent validates outside the   *)
(*   lock; the loser reports "does not need a restart". Loom P3h/P3i.   *)
(***************************************************************************)

SingleRunner == (cur # NoAct => busy) /\ ((busy /\ cur = NoAct) => shut)
ParkedClean == failed => (~busy /\ cur = NoAct)
IdleCleanEmpty == (~busy /\ ~failed) => queue = <<>>
ShutLatch == shut <=> shutAt >= 0
ShutAtBound == shut => shutAt <= Len(accepted)
FailedReject == Disjoint(rej, LogSet \union QueueSet \union CurSet \union dropped \union gone)
NestedNeverRejected == \A b \in AcceptedSet : b[3] = "nested" => b \notin rej
FifoOrder == IsSubseq(log, accepted)
EachOnce == NoDupSeq(log) /\ NoDupSeq(queue) /\ NoDupSeq(accepted)
HeldAccounted ==
    /\ AcceptedSet \subseteq LogSet \union QueueSet \union CurSet \union dropped \union gone
    /\ Disjoint(LogSet, QueueSet) /\ Disjoint(LogSet, CurSet)
    /\ Disjoint(LogSet, dropped) /\ Disjoint(LogSet, gone)
    /\ Disjoint(QueueSet, CurSet) /\ Disjoint(QueueSet, dropped)
    /\ Disjoint(QueueSet, gone) /\ Disjoint(CurSet, dropped)
    /\ Disjoint(CurSet, gone) /\ Disjoint(dropped, gone)
DroppedAfterShutdown == dropped = {} \/ shut
PostShutdownNeverRuns ==
    shut => \A i \in (shutAt + 1) .. Len(accepted) : accepted[i] \notin LogSet
AtMostOneOk == Cardinality({r \in Restarters : result[r] = "ok"}) <= 1
OkImpliesClean == (\E r \in Restarters : result[r] = "ok") => ~failed

Safety ==
    /\ SingleRunner
    /\ ParkedClean
    /\ IdleCleanEmpty
    /\ ShutLatch
    /\ ShutAtBound
    /\ FailedReject
    /\ NestedNeverRejected
    /\ FifoOrder
    /\ EachOnce
    /\ HeldAccounted
    /\ DroppedAfterShutdown
    /\ PostShutdownNeverRuns
    /\ AtMostOneOk
    /\ OkImpliesClean

THEOREM FairSpec => []TypeOK
THEOREM FairSpec => []Safety
THEOREM FairSpec => Liveness

=============================================================================
