(* Copyright (c) Rich Hickey. All rights reserved. *)
(* The use and distribution terms for this software are covered by the *)
(* Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php) *)
(* which can be found in the file epl-v10.html at the root of this distribution. *)
(* By using this software in any fashion, you are agreeing to be bound by *)
(* the terms of this license. *)
(* You must not remove this notice, or any other, from this software. *)

---- MODULE channel_mailbox ----
(***************************************************************************)
(* #188: core.async channel mailbox core (blocking side, plain ops).       *)
(*                                                                         *)
(* Models ONE channel plus its put/take/close protocol, abstracting the   *)
(* oracle probes in docs/gap-123/g123-chan.clj, the mailbox in             *)
(* rust/crates/clj-value/src/channel.rs (take_one, put_one, close,        *)
(* ChanState::{refill, serve_takes_all}), and the attempt-then-wait       *)
(* drivers in rust/crates/clj-eval/src/async.rs (native_put_block,        *)
(* native_take_block). Mirrors the loom model in                          *)
(* rust/crates/clj-value/tests/loom_channel.rs at the mailbox-protocol    *)
(* level (the loom mirror owns the park/wakeup schedule exploration --    *)
(* lost wakeups, condvar discipline, the channel-then-waiter lock order;  *)
(* this spec owns buffer accounting, rendezvous/FIFO order, and           *)
(* close-release).                                                         *)
(*                                                                         *)
(* TLC FILE-NAME NOTE: TLA+ module names cannot contain hyphens, so     *)
(* this file must stay named channel_mailbox.tla (basename == module name). *)
(*                                                                         *)
(* Run (from docs/t1174/models/):                                          *)
(*   java -cp tla2tools.jar tlc2.TLC -workers auto -cleanup \               *)
(*     -config channel_mailbox.cfg channel_mailbox.tla                     *)
(*   java -cp tla2tools.jar tlc2.TLC -workers auto -cleanup \               *)
(*     -config channel_mailbox_cap0.cfg channel_mailbox.tla                *)
(*                                                                         *)
(* SCOPE (what is modeled):                                                *)
(*   - put/take/close on one channel: takes drain the buffer first        *)
(*     (refilling from queued putters), then rendezvous with queued        *)
(*     putters, then nil when closed, else park; puts rendezvous with     *)
(*     queued takers first, then false when closed, then buffer, else     *)
(*     park (TakeTakeOnce-style order in Put/Take).                       *)
(*   - fixed/dropping/sliding buffer accounting: fixed blocks when full   *)
(*     (BufBounded); dropping pushes while room last and drops the rest  *)
(*     (DroppedOnlyDropping); sliding evicts the oldest on a full        *)
(*     integral buffer (EvictedOnlySliding); a non-integral sliding      *)
(*     size never evicts (the JVM `(= size n)` quirk -- unbounded here,  *)
(*     bounded only by the finite script); sliding-zero evicting from    *)
(*     empty errors (EvictErrorOnlySlidingZero, CAP=0 config).            *)
(*   - close-release wakeups: close serves every queued taker (buffered   *)
(*     values first, else nil) and leaves every queued put parked until  *)
(*     a taker releases it (ClosedTakesEmpty, PutsParkedIffQueued -- no  *)
(*     parked put ever resolves false/error at close).                   *)
(*   - FIFO: taken values are a subsequence of accepted puts in accept    *)
(*     order (TakenSubseqAccepted), across rendezvous, refill, and       *)
(*     close-serve; every accepted value sits in exactly one of the      *)
(*     puts queue / buf / taken / dropped / evicted (Conservation).      *)
(*   - queue-bound errors: the MAX-QUEUE-SIZE check placement (after      *)
(*     rendezvous/closed/buffer, before parking) with MAX scaled to 2    *)
(*     (ErrorOnlyWhenFull).                                               *)
(*                                                                         *)
(* NO LIVENESS (deliberate, unlike agent_queue): puts and takes BLOCK by  *)
(* design -- an unbuffered put with no taker parks forever -- so no       *)
(* non-blocking property holds. Terminating is an unconditional stutter:  *)
(* a parked op with no counterpart is legitimate blocking, not a wedge.   *)
(* The deadlock check is therefore vacuous by design; Safety carries     *)
(* every pin.                                                             *)
(*                                                                         *)
(* DELIBERATE ABSTRACTIONS (gaps vs the Rust code):                        *)
(*   G1 transducers: put_xform/close_xform/complete_closed, the xform     *)
(*      gate, overfilling add!, completion flushes, and reduced-abort     *)
(*      are NOT modeled (explicit follow-up). In particular the           *)
(*      close-serve-buffered arm below is unreachable in plain            *)
(*      behaviors (TakesBufExclusive: takes queue only behind an empty   *)
(*      buffer, so plain close always serves nil); it stays for fidelity *)
(*      with serve_takes_all.                                             *)
(*   G2 alts!!: the shared-waiter first-wins commit, the :default kill    *)
(*      path, and stale-reg sweeps are NOT modeled (explicit              *)
(*      follow-up). Plain ops mint a fresh waiter per call that never    *)
(*      goes stale while parked, so sweep/next_taker/next_putter         *)
(*      collapse to plain head pops and every claim-first scan wins on   *)
(*      the first live reg.                                               *)
(*   G3 waiter/condvar mechanics: park is "append a reg", wakeup is      *)
(*      "remove the reg and record the outcome", both inside the same    *)
(*      atomic under-lock attempt as the real code's try_win. Lost       *)
(*      wakeups, spurious wakeups, and the channel-then-waiter lock      *)
(*      order are NOT modeled -- the loom mirror owns those.             *)
(*   G4 queue bound: MAX-QUEUE-SIZE 1024 scales to MAX=2 (both error      *)
(*      arms reachable with 2x2 scripts); the bound value itself is      *)
(*      pinned by the in-module tests.                                    *)
(*   G5 values: puts carry <<putter, seq>> ids; nil-put rejection lives  *)
(*      in clj-eval (native_put_block), payloads never steer the mailbox. *)
(*   G6 go-blocks: go-block settle paths are NOT modeled (follow-up).     *)
(***************************************************************************)

EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    Putters, \* Concurrent putter threads, e.g. {p1, p2}
    Takers,  \* Concurrent taker threads, e.g. {t1, t2}
    K,       \* Puts per putter (script length; blocking: next issues only
             \* after the previous resolves -- the >!! wait)
    M,       \* Takes per taker (likewise blocking -- the <!! wait)
    CAP,     \* Buffer capacity (main cfg 1; cap0 cfg 0)
    MAX      \* Queue bound (MAX-QUEUE-SIZE scaled to 2)

\* A put value is <<putter, seq>>; a take op is <<taker, seq>>.
ValueSet == Putters \X (1 .. K)
TakeOpSet == Takers \X (1 .. M)

VARIABLES
    buf,       \* Buffered values, head first
    kind,      \* "unbuf" | "fixed" | "dropping" | "sliding"
    integral,  \* Sliding size integral? (only read when sliding)
    closed,    \* Close latch
    takes,     \* Queued take regs, head first
    puts,      \* Queued put regs (values), head first
    issuedP,   \* [Putters -> 0..K] puts issued (attempted)
    issuedT,   \* [Takers -> 0..M] takes issued
    pOut,      \* [ValueSet -> outcome]: "none" (unissued) | "parked"
               \* | "true" | "false" (closed) | "qfull" | "eevict"
    tOut,      \* [TakeOpSet -> outcome]: <<"none">> (unissued) |
               \* <<"parked">> | <<"val", v>> | <<"nil">> | <<"qfull">>
               \* (tagged tuples throughout: TLC errors comparing a
               \* string to a non-string, so bare strings cannot mix
               \* with values here)
    pErrLen,   \* [ValueSet -> Nat] puts depth recorded at a qfull error
    tErrLen,   \* [TakeOpSet -> Nat] takes depth recorded at a qfull error
    accepted,  \* Puts accepted (true now or true later), in accept order
    taken,     \* Values delivered to takes, in completion order
    dropped,   \* Dropping-overflow values (put still true)
    evicted    \* Sliding-evicted values (put still true)

vars == <<buf, kind, integral, closed, takes, puts, issuedP, issuedT,
          pOut, tOut, pErrLen, tErrLen, accepted, taken, dropped, evicted>>

(***************************************************************************)
(* Helpers.                                                                *)
(***************************************************************************)

BufSet == { buf[i] : i \in DOMAIN buf }
PutsSet == { puts[i] : i \in DOMAIN puts }
TakesSet == { takes[i] : i \in DOMAIN takes }
AcceptedSet == { accepted[i] : i \in DOMAIN accepted }
TakenSet == { taken[i] : i \in DOMAIN taken }

RECURSIVE IsSubseq(_, _)
IsSubseq(a, b) ==
    IF a = <<>> THEN TRUE
    ELSE IF b = <<>> THEN FALSE
    ELSE IF Head(a) = Head(b) THEN IsSubseq(Tail(a), Tail(b))
    ELSE IsSubseq(a, Tail(b))

NoDupSeq(s) == Cardinality({ s[i] : i \in DOMAIN s }) = Len(s)

Disjoint(A, B) == A \cap B = {}

\* Refill(b, q): move queued putters into freed buffer room, head
\* first, while room lasts (ChanState::refill; puts only ever queue
\* behind fixed/unbuffered, but the Dropping arm mirrors
\* refill_room's match for fidelity). Returns
\* <<buf, puts, moved>>. Bounded: q only shrinks.
RECURSIVE Refill(_, _)
Refill(b, q) ==
    IF q # <<>> /\ kind \in {"fixed", "dropping"} /\ (Len(b) < CAP)
    THEN LET moved == Refill(Append(b, Head(q)), Tail(q))
         IN <<moved[1], moved[2], <<Head(q)>> \o moved[3]>>
    ELSE <<b, q, <<>>>>

\* Serve(b, q): hand queued takers buffered values (one each, head
\* first), then nil to the rest (ChanState::serve_takes_all; plain
\* waiters always win, so peek-then-pop collapses to pop-then-win).
\* Returns [buf |-> buf, outs |-> << <<takeop, outcome>> ... >>,
\* vals |-> served values in order]. Bounded: q only shrinks.
RECURSIVE Serve(_, _)
Serve(b, q) ==
    IF q = <<>> THEN [buf |-> b, outs |-> <<>>, vals |-> <<>>]
    ELSE LET v == IF b = <<>> THEN <<"nil">> ELSE <<"val", Head(b)>> IN
         LET b2 == IF b = <<>> THEN b ELSE Tail(b) IN
         LET r == Serve(b2, Tail(q)) IN
         [buf |-> r.buf,
          outs |-> << <<Head(q), v>> >> \o r.outs,
          vals |-> (IF v = <<"nil">> THEN <<>> ELSE <<v[2]>>) \o r.vals]

\* Apply a <<key, val>> sequence to a function, head first.
RECURSIVE ApplyOuts(_, _)
ApplyOuts(f, s) ==
    IF s = <<>> THEN f
    ELSE ApplyOuts([f EXCEPT ![Head(s)[1]] = Head(s)[2]], Tail(s))

\* Resolve a moved-putter sequence to "true" in pOut, head first.
RECURSIVE ResolveMoved(_, _)
ResolveMoved(f, s) ==
    IF s = <<>> THEN f
    ELSE ResolveMoved([f EXCEPT ![Head(s)] = "true"], Tail(s))

\* Blocking-script guards: the previous op resolved (IF short-circuits;
\* `issuedP[p] = 0 \/ ...' does NOT -- TLC evaluates both disjuncts).
PrevPutResolved(p) ==
    IF issuedP[p] = 0 THEN TRUE ELSE pOut[<<p, issuedP[p]>>] # "parked"
PrevTakeResolved(t) ==
    IF issuedT[t] = 0 THEN TRUE ELSE tOut[<<t, issuedT[t]>>] # <<"parked">>

(***************************************************************************)
(* Type invariant.                                                         *)
(***************************************************************************)

TypeOK ==
    /\ buf \in Seq(ValueSet)
    /\ kind \in {"unbuf", "fixed", "dropping", "sliding"}
    /\ integral \in BOOLEAN
    /\ closed \in BOOLEAN
    /\ takes \in Seq(TakeOpSet)
    /\ puts \in Seq(ValueSet)
    /\ issuedP \in [Putters -> 0 .. K]
    /\ issuedT \in [Takers -> 0 .. M]
    /\ pOut \in [ValueSet -> {"none", "parked", "true", "false",
                              "qfull", "eevict"}]
    /\ tOut \in [TakeOpSet -> ({<<"none">>, <<"parked">>, <<"nil">>,
                                     <<"qfull">>}
                                    \union {<<"val", v>> : v \in ValueSet})]
    /\ pErrLen \in [ValueSet -> Nat]
    /\ tErrLen \in [TakeOpSet -> Nat]
    /\ accepted \in Seq(ValueSet)
    /\ taken \in Seq(ValueSet)
    /\ dropped \in SUBSET ValueSet
    /\ evicted \in SUBSET ValueSet

(***************************************************************************)
(* Initial state. `kind' and `integral' are chosen nondeterministically so *)
(* one TLC run covers all four buffer kinds and both sliding modes (fixed *)
(* per behavior).                                                          *)
(***************************************************************************)

Init ==
    /\ buf = <<>>
    /\ kind \in {"unbuf", "fixed", "dropping", "sliding"}
    \* `integral' is read only on the sliding arm, so non-sliding
    \* behaviors fix it TRUE (halves those legs, zero coverage loss).
    /\ integral \in IF kind = "sliding" THEN BOOLEAN ELSE {TRUE}
    /\ closed = FALSE
    /\ takes = <<>>
    /\ puts = <<>>
    /\ issuedP = [p \in Putters |-> 0]
    /\ issuedT = [t \in Takers |-> 0]
    /\ pOut = [v \in ValueSet |-> "none"]
    /\ tOut = [o \in TakeOpSet |-> <<"none">>]
    /\ pErrLen = [v \in ValueSet |-> 0]
    /\ tErrLen = [o \in TakeOpSet |-> 0]
    /\ accepted = <<>>
    /\ taken = <<>>
    /\ dropped = {}
    /\ evicted = {}

(***************************************************************************)
(* Put(p): Channel::put_one for one blocking put. The putter issues its   *)
(* next put only after the previous resolves (the >!! wait on its fresh   *)
(* waiter). Order: rendezvous with a queued taker, false when closed,     *)
(* buffer by kind, else queue-or-qfull (G2: the claim-first scan wins on  *)
(* the head reg; G4: MAX-QUEUE-SIZE scaled to MAX).                       *)
(***************************************************************************)

Put(p) ==
    /\ issuedP[p] < K
    /\ PrevPutResolved(p)
    /\ LET v == <<p, issuedP[p] + 1>> IN
        /\ issuedP' = [issuedP EXCEPT ![p] = issuedP[p] + 1]
        /\ IF takes # <<>>
           THEN (* Rendezvous: the head taker wins v at once. *)
                /\ LET o == Head(takes) IN
                    /\ takes' = Tail(takes)
                    /\ tOut' = [tOut EXCEPT ![o] = <<"val", v>>]
                    /\ taken' = taken \o <<v>>
                /\ pOut' = [pOut EXCEPT ![v] = "true"]
                /\ accepted' = accepted \o <<v>>
                /\ UNCHANGED <<buf, puts, dropped, evicted, pErrLen>>
           ELSE IF closed
                THEN (* Closed: puts fail false (queued takers would have *)
                     (* rendezvoused above; post-close takes never queue, *)
                     (* so none are queued -- ClosedTakesEmpty).          *)
                     /\ pOut' = [pOut EXCEPT ![v] = "false"]
                     /\ UNCHANGED <<buf, takes, puts, tOut, taken,
                                    accepted, dropped, evicted, pErrLen>>
                ELSE IF kind = "fixed" /\ Len(buf) < CAP
                     THEN (* Fixed with room: buffer. *)
                          /\ buf' = Append(buf, v)
                          /\ pOut' = [pOut EXCEPT ![v] = "true"]
                          /\ accepted' = accepted \o <<v>>
                          /\ UNCHANGED <<takes, puts, tOut, taken,
                                         dropped, evicted, pErrLen>>
                     ELSE IF kind = "dropping"
                          THEN (* Dropping never blocks: buffer while room *)
                               (* lasts, drop the overflow (still true).  *)
                               /\ IF Len(buf) < CAP
                                  THEN /\ buf' = Append(buf, v)
                                       /\ UNCHANGED dropped
                                  ELSE /\ dropped' = dropped \union {v}
                                       /\ UNCHANGED buf
                               /\ pOut' = [pOut EXCEPT ![v] = "true"]
                               /\ accepted' = accepted \o <<v>>
                               /\ UNCHANGED <<takes, puts, tOut, taken,
                                              evicted, pErrLen>>
                          ELSE IF kind = "sliding"
                               THEN (* Sliding never blocks: evict the    *)
                                    (* oldest on a full integral buffer  *)
                                    (* (the JVM `(= size n)` quirk means *)
                                    (* a non-integral size never evicts);*)
                                    (* evicting from empty errors       *)
                                    (* (sliding-zero -- the JVM's        *)
                                    (* LinkedList.removeLast crash).     *)
                                    /\ IF integral /\ Len(buf) = CAP
                                       THEN IF buf = <<>>
                                            THEN (* Sliding-zero evict. *)
                                                 /\ pOut' = [pOut EXCEPT
                                                               ![v] = "eevict"]
                                                 /\ UNCHANGED <<buf, takes, puts,
                                                     tOut, taken, accepted,
                                                     dropped, evicted>>
                                            ELSE /\ evicted' = evicted
                                                          \union {Head(buf)}
                                                 /\ buf' = Tail(buf) \o <<v>>
                                                 /\ pOut' = [pOut EXCEPT
                                                               ![v] = "true"]
                                                 /\ accepted' = accepted \o <<v>>
                                                 /\ UNCHANGED <<takes, puts, tOut,
                                                     taken, dropped>>
                                       ELSE /\ buf' = Append(buf, v)
                                            /\ pOut' = [pOut EXCEPT
                                                          ![v] = "true"]
                                            /\ accepted' = accepted \o <<v>>
                                            /\ UNCHANGED <<takes, puts, tOut, taken,
                                                 dropped, evicted>>
                                    /\ UNCHANGED pErrLen
                               ELSE (* Fixed-full or unbuffered: queue, or *)
                                    (* qfull past MAX (G4).              *)
                                    /\ IF Len(puts) >= MAX
                                       THEN /\ pOut' = [pOut EXCEPT
                                                          ![v] = "qfull"]
                                            /\ pErrLen' = [pErrLen EXCEPT
                                                             ![v] = Len(puts)]
                                            /\ UNCHANGED <<puts, accepted>>
                                       ELSE /\ puts' = Append(puts, v)
                                            /\ pOut' = [pOut EXCEPT
                                                          ![v] = "parked"]
                                            /\ accepted' = accepted \o <<v>>
                                            /\ UNCHANGED pErrLen
                                    /\ UNCHANGED <<buf, takes, tOut, taken,
                                                 dropped, evicted>>
    /\ UNCHANGED <<kind, integral, closed, issuedT, tErrLen>>

(***************************************************************************)
(* Take(t): Channel::take_one for one blocking take. Order: drain the     *)
(* buffer (refilling from queued putters), rendezvous with a queued      *)
(* putter, nil when closed, else queue-or-qfull (G2: rendezvous claims   *)
(* the head putter; refill moves head-first while room lasts).           *)
(***************************************************************************)

Take(t) ==
    /\ issuedT[t] < M
    /\ PrevTakeResolved(t)
    /\ LET o == <<t, issuedT[t] + 1>> IN
        /\ issuedT' = [issuedT EXCEPT ![t] = issuedT[t] + 1]
        /\ IF buf # <<>>
           THEN (* Drain the head, refilling freed room head-first. *)
                LET r == Refill(Tail(buf), puts) IN
                /\ tOut' = [tOut EXCEPT ![o] = <<"val", Head(buf)>>]
                /\ taken' = taken \o <<Head(buf)>>
                /\ buf' = r[1]
                /\ puts' = r[2]
                /\ pOut' = ResolveMoved(pOut, r[3])
                /\ UNCHANGED <<takes, accepted, dropped, evicted, tErrLen>>
           ELSE IF puts # <<>>
                THEN (* Rendezvous: the head putter completes true (no   *)
                     (* refill on this path -- the buffer stays empty).  *)
                     /\ LET w == Head(puts) IN
                         /\ puts' = Tail(puts)
                         /\ pOut' = [pOut EXCEPT ![w] = "true"]
                         /\ tOut' = [tOut EXCEPT ![o] = <<"val", w>>]
                         /\ taken' = taken \o <<w>>
                     /\ UNCHANGED <<buf, takes, accepted, dropped,
                                    evicted, tErrLen>>
                ELSE IF closed
                     THEN (* Closed and drained: nil. *)
                          /\ tOut' = [tOut EXCEPT ![o] = <<"nil">>]
                          /\ UNCHANGED <<buf, takes, puts, pOut, taken,
                                         accepted, dropped, evicted, tErrLen>>
                     ELSE IF Len(takes) >= MAX
                          THEN (* Queue bound (G4). *)
                               /\ tOut' = [tOut EXCEPT ![o] = <<"qfull">>]
                               /\ tErrLen' = [tErrLen EXCEPT
                                                ![o] = Len(takes)]
                               /\ UNCHANGED <<buf, takes, puts, pOut, taken,
                                              accepted, dropped, evicted>>
                          ELSE (* Park. *)
                               /\ takes' = Append(takes, o)
                               /\ tOut' = [tOut EXCEPT ![o] = <<"parked">>]
                               /\ UNCHANGED <<buf, puts, pOut, taken, accepted,
                                              dropped, evicted, tErrLen>>
    /\ UNCHANGED <<kind, integral, closed, issuedP, pErrLen>>

(***************************************************************************)
(* Close: Channel::close (idempotent). Latches, then serves every queued  *)
(* taker a buffered value (head first) else nil. Queued puts stay parked *)
(* until a taker releases them (PutsParkedIffQueued). A second close is  *)
(* a no-op by construction, so one guarded action covers racing closes.  *)
(* (G1: the takes-queued-behind-buffered arm is unreachable on plain     *)
(* channels -- TakesBufExclusive -- and stays for serve_takes_all       *)
(* fidelity.)                                                             *)
(***************************************************************************)

Close ==
    /\ ~closed
    /\ closed' = TRUE
    /\ LET r == Serve(buf, takes) IN
        /\ buf' = r.buf
        /\ takes' = <<>>
        /\ tOut' = ApplyOuts(tOut, r.outs)
        /\ taken' = taken \o r.vals
    /\ UNCHANGED <<kind, integral, puts, issuedP, issuedT, pOut,
                   pErrLen, tErrLen, accepted, dropped, evicted>>

(***************************************************************************)
(* Terminating: unconditional stutter. A parked put/take with no          *)
(* counterpart blocks FOREVER by design (unlike agent sends, channel ops *)
(* are blocking), so the end of an issuable script is not a wedge -- any *)
(* behavior may stop issuing. The TLC deadlock check is therefore        *)
(* vacuous by design; Safety carries every pin.                           *)
(***************************************************************************)

Terminating ==
    UNCHANGED vars

Next ==
    \/ \E p \in Putters : Put(p)
    \/ \E t \in Takers : Take(t)
    \/ Close
    \/ Terminating

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* INVARIANT MAP -- each Safety conjunct -> the Rust path implementing it. *)
(*                                                                         *)
(* TakesBufExclusive: takes = <<>> \/ buf = <<>>                          *)
(*   A queued take implies an empty buffer. -> channel.rs: take_one pops  *)
(*   the buffer before parking; put_one rendezvouses with queued takers  *)
(*   before buffering; refill only runs on the drain path (whose buffer  *)
(*   was nonempty, hence taker-free). (G2: stale alts regs are swept at  *)
(*   every attempt, restoring this at every action boundary.)             *)
(* PutsTakesExclusive: puts = <<>> \/ takes = <<>>                        *)
(*   Puts and takes never queue together. -> channel.rs: puts queue only *)
(*   past the rendezvous scan (takes empty); takes queue only past the   *)
(*   putter scan (puts empty).                                            *)
(* ClosedTakesEmpty: closed => takes = <<>>                               *)
(*   Close drains every queued taker and post-close takes never park. -> *)
(*   channel.rs: close serve_takes_all; take_one's closed arm precedes   *)
(*   the park.                                                            *)
(* BufBounded: fixed/dropping/integral-sliding bufs never exceed CAP.    *)
(*   -> channel.rs: fixed pushes guarded by full?; dropping pushes      *)
(*   guarded, overflow dropped; sliding evicts-then-pushes on full      *)
(*   integral; refill_room re-checks per move. Non-integral sliding is   *)
(*   unbounded by the JVM `(= size n)` quirk (no bound claimed).         *)
(* EachOnce: buf/puts/accepted/taken hold no duplicates.                 *)
(*   -> every put value moves BY REG (queued once, popped once, served  *)
(*   once); loom rendezvous/FIFO pins.                                   *)
(* Conservation: every accepted value sits in exactly one of the puts   *)
(*   queue / buf / taken / dropped / evicted, and every live value was  *)
(*   accepted. -> the ownership discipline across take_one, put_one,    *)
(*   refill, and serve_takes_all; dropped/evicted complete true but are  *)
(*   never delivered.                                                    *)
(* TakenSubseqAccepted: taken is a subsequence of accepted (FIFO).      *)
(*   -> channel.rs: buf and puts are VecDeque FIFOs; takes always drain *)
(*   the buffer (refilling head-first) before rendezvousing head-first; *)
(*   close serves head-first.                                           *)
(* PutFalseOnlyClosed / RejectedNeverAccepted: a false put implies the  *)
(*   latch, and false/error puts never enter the system. -> channel.rs: *)
(*   put_one's closed arm; >!! surfaces false, QueueFull/EmptyEvict    *)
(*   throw past chan_err.                                               *)
(* PutsParkedIffQueued / TakesParkedIffQueued: "parked" <=> reg queued. *)
(*   Close resolves takers but NEVER puts: a parked put still sits in   *)
(*   the puts queue after close, completing true only when a taker      *)
(*   claims it (refill or rendezvous). -> channel.rs: close serves     *)
(*   takes only; take_one's buf/refill/rendezvous arms resolve puts.   *)
(*   Loom close-release pins.                                           *)
(* NilTakeOnlyClosed: a nil take implies the latch. -> channel.rs:     *)
(*   take_one's closed arm; close serves nil only past the buffer.      *)
(* ErrorOnlyWhenFull: every qfull error recorded a full queue.         *)
(*   -> channel.rs: the MAX-QUEUE-SIZE check sits after               *)
(*   rendezvous/closed/buffer and before parking (G4: MAX=2 here).     *)
(* EvictErrorOnlySlidingZero: eevict implies sliding + integral +     *)
(*   CAP=0. -> channel.rs: put_one's sliding arm pops from empty only *)
(*   when `(= size n)` holds at size 0 (oracle EmptyEvict).            *)
(* DroppedOnlyDropping / EvictedOnlySliding: overflow sets imply their *)
(*   kinds. -> channel.rs: the dropping/sliding buffer arms.           *)
(* OutcomeIssued: outcomes exist exactly for issued ops (no phantoms). *)
(*   -> the attempt-then-wait drivers record exactly one outcome per   *)
(*   call (async.rs native_put_block/native_take_block).               *)
(***************************************************************************)

TakesBufExclusive == takes = <<>> \/ buf = <<>>
PutsTakesExclusive == puts = <<>> \/ takes = <<>>
ClosedTakesEmpty == closed => takes = <<>>
BufBounded ==
    /\ kind = "fixed" => Len(buf) <= CAP
    /\ kind = "dropping" => Len(buf) <= CAP
    /\ (kind = "sliding" /\ integral) => Len(buf) <= CAP
EachOnce ==
    /\ NoDupSeq(buf)
    /\ NoDupSeq(puts)
    /\ NoDupSeq(accepted)
    /\ NoDupSeq(taken)
Conservation ==
    /\ AcceptedSet \subseteq PutsSet \union BufSet \union TakenSet
          \union dropped \union evicted
    /\ PutsSet \union BufSet \union TakenSet \subseteq AcceptedSet
    /\ dropped \subseteq AcceptedSet
    /\ evicted \subseteq AcceptedSet
    /\ Disjoint(PutsSet, BufSet) /\ Disjoint(PutsSet, TakenSet)
    /\ Disjoint(PutsSet, dropped) /\ Disjoint(PutsSet, evicted)
    /\ Disjoint(BufSet, TakenSet) /\ Disjoint(BufSet, dropped)
    /\ Disjoint(BufSet, evicted) /\ Disjoint(TakenSet, dropped)
    /\ Disjoint(TakenSet, evicted) /\ Disjoint(dropped, evicted)
TakenSubseqAccepted == IsSubseq(taken, accepted)
PutFalseOnlyClosed == \A v \in ValueSet : pOut[v] = "false" => closed
RejectedNeverAccepted ==
    \A v \in ValueSet :
        pOut[v] \in {"false", "qfull", "eevict"} => v \notin AcceptedSet
PutsParkedIffQueued ==
    \A v \in ValueSet : (pOut[v] = "parked") <=> (v \in PutsSet)
TakesParkedIffQueued ==
    \A o \in TakeOpSet : (tOut[o] = <<"parked">>) <=> (o \in TakesSet)
NilTakeOnlyClosed == \A o \in TakeOpSet : tOut[o] = <<"nil">> => closed
ErrorOnlyWhenFull ==
    /\ \A v \in ValueSet : pOut[v] = "qfull" => pErrLen[v] >= MAX
    /\ \A o \in TakeOpSet : tOut[o] = <<"qfull">> => tErrLen[o] >= MAX
EvictErrorOnlySlidingZero ==
    \A v \in ValueSet :
        pOut[v] = "eevict" => kind = "sliding" /\ integral /\ CAP = 0
DroppedOnlyDropping == dropped # {} => kind = "dropping"
EvictedOnlySliding == evicted # {} => kind = "sliding"
OutcomeIssued ==
    /\ \A p \in Putters : \A n \in 1 .. K :
        (pOut[<<p, n>>] # "none") <=> (issuedP[p] >= n)
    /\ \A t \in Takers : \A n \in 1 .. M :
        (tOut[<<t, n>>] # <<"none">>) <=> (issuedT[t] >= n)

Safety ==
    /\ TakesBufExclusive
    /\ PutsTakesExclusive
    /\ ClosedTakesEmpty
    /\ BufBounded
    /\ EachOnce
    /\ Conservation
    /\ TakenSubseqAccepted
    /\ PutFalseOnlyClosed
    /\ RejectedNeverAccepted
    /\ PutsParkedIffQueued
    /\ TakesParkedIffQueued
    /\ NilTakeOnlyClosed
    /\ ErrorOnlyWhenFull
    /\ EvictErrorOnlySlidingZero
    /\ DroppedOnlyDropping
    /\ EvictedOnlySliding
    /\ OutcomeIssued

THEOREM Spec => []TypeOK
THEOREM Spec => []Safety

=============================================================================
