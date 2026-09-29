package freechips.rocketchip.rocket

import chisel3._
import chisel3.util._
import freechips.rocketchip.tile.{CoreBundle, CoreModule}
import org.chipsalliance.cde.config.Parameters

class ThreadContextManagerIO(implicit p: Parameters) extends CoreBundle()(p) {
  val tpt_read_req = Decoupled(new ThreadTptReadReq)
  val tpt_read_resp = Flipped(Valid(new ThreadTptReadResp))
  val tpt_ready_chunks =
    Input(Vec((threadSupport + 63) / 64, UInt(64.W)))
  val slot_meta = Input(Vec(threadSlotCount, new ThreadSchedMeta))
  val current_thread = Input(UInt(threadIdLength.W))

  val can_start = Input(Bool())
  val ctx_write_busy = Input(Bool())
  val saver_busy_slots = Input(UInt(threadSlotCount.W))
  val save_done = Input(Valid(new ThreadContextOp))
  val load_done = Input(Valid(new ThreadContextOp))
  val slot_load_request = Output(Valid(new ThreadContextOp))
  val slot_load_commit = Input(Valid(new ThreadSlotLoadCommit))
  val slot_drain_busy = Input(UInt(threadSlotCount.W))
  val slot_dirty_set = Input(Valid(UInt(threadSlotIdLength.W)))
  val rank_change = Flipped(Decoupled(new ThreadRankChange))
  val metadata_pending = Input(Bool())

  val save_start = Output(Valid(new ThreadContextOp))
  val load_start = Output(Valid(new ThreadContextOp))
  val load_redirect = Decoupled(new ThreadContextOp)
  val kill_thread = Output(Valid(UInt(threadIdLength.W)))

  val slot_thread = Output(Vec(threadSlotCount, UInt(threadIdLength.W)))
  val slot_locked = Output(UInt(threadSlotCount.W))
  val slot_sched_blocked = Output(UInt(threadSlotCount.W))
  val slot_ctx_write_blocked = Output(UInt(threadSlotCount.W))
  val staged_valid = Output(Bool())
  val staged_thread = Output(UInt(threadIdLength.W))
}

class ThreadContextManager(implicit p: Parameters) extends CoreModule {
  val io = IO(new ThreadContextManagerIO)

  private def threadletPrintf(body: => Unit): Unit =
    if (coreParams.threadletAreaDebugPrintf) { body }

  private def threadletAssert(cond: => Bool, message: String): Unit =
    if (coreParams.threadletAreaDebugAssert) { assert(cond, message) }

  private val initialResidentCount = threadSupport min threadSlotCount
  private val scanChunkBits = 64
  private val scanChunkCount = (threadSupport + scanChunkBits - 1) / scanChunkBits
  require(threadSupport % scanChunkBits == 0,
    "ThreadContextManager scanner requires complete 64-thread chunks")

  private def bit(mask: UInt, idx: UInt): Bool = VecInit(mask.asBools)(idx)

  private def threadOH(thread: UInt): UInt =
    UIntToOH(thread, threadSupport)(threadSupport - 1, 0)

  private def rrDistanceAfter(base: UInt, tid: UInt): UInt = {
    val distBits = threadIdLength + 1
    val tidExt = Cat(0.U(1.W), tid)
    val baseExt = Cat(0.U(1.W), base)
    val forward = tidExt - baseExt
    val wrapped = tidExt + threadSupport.U(distBits.W) - baseExt
    Mux(tid > base, forward, wrapped)(distBits - 1, 0)
  }

  private def zeroSchedMeta: ThreadSchedMeta =
    0.U.asTypeOf(new ThreadSchedMeta)

  private def muxMeta(sel: Bool, a: ThreadSchedMeta, b: ThreadSchedMeta): ThreadSchedMeta =
    Mux(sel, a.asUInt, b.asUInt).asTypeOf(new ThreadSchedMeta)

  private def rankHigherMeta(a: UInt, aMeta: ThreadSchedMeta, b: UInt, bMeta: ThreadSchedMeta): Bool = {
    val a_valid = aMeta.valid
    val b_valid = bMeta.valid
    val a_runnable = aMeta.runable
    val b_runnable = bMeta.runable
    val a_prio = aMeta.priority
    val b_prio = bMeta.priority
    val a_deadline = aMeta.deadline
    val b_deadline = bMeta.deadline
    val a_deadline_set = a_deadline =/= 0.U
    val b_deadline_set = b_deadline =/= 0.U
    val a_dist = rrDistanceAfter(io.current_thread, a)
    val b_dist = rrDistanceAfter(io.current_thread, b)

    a_valid && (!b_valid ||
      (a_runnable && !b_runnable) ||
      ((a_runnable === b_runnable) &&
        ((a_prio > b_prio) ||
         ((a_prio === b_prio) &&
           ((a_deadline_set && !b_deadline_set) ||
            (a_deadline_set && b_deadline_set &&
              ((a_deadline < b_deadline) ||
               ((a_deadline === b_deadline) && (a_dist < b_dist)))) ||
            (!a_deadline_set && !b_deadline_set && (a_dist < b_dist)))))))
  }

  private def schedReadyMeta(meta: ThreadSchedMeta): Bool =
    meta.valid && meta.runable

  val slot_thread = RegInit(VecInit((0 until threadSlotCount).map(_.U(threadIdLength.W))))
  private val initialResidentMask =
    ((BigInt(1) << initialResidentCount) - 1).U(threadSupport.W)
  val thread_resident = RegInit(initialResidentMask)
  val slot_dirty = RegInit(VecInit(Seq.fill(threadSlotCount)(true.B)))
  val slot_replacing = RegInit(VecInit(Seq.fill(threadSlotCount)(false.B)))

  val (s_idle :: s_drain :: s_save_wait :: s_load_start :: s_load_wait ::
    s_load_metadata_wait :: s_load_commit_wait :: Nil) = Enum(7)
  val state = RegInit(s_idle)
  val rep_slot = RegInit(0.U(threadSlotIdLength.W))
  val rep_victim = RegInit(0.U(threadIdLength.W))
  val rep_staged = RegInit(0.U(threadIdLength.W))
  val rep_victim_meta = RegInit(zeroSchedMeta)
  val rep_staged_meta = RegInit(zeroSchedMeta)
  val staged_valid_reg = RegInit(false.B)
  val staged_thread_reg = RegInit(0.U(threadIdLength.W))
  val staged_meta_reg = RegInit(zeroSchedMeta)
  val scan_active = RegInit(false.B)
  val scan_candidate_chunks =
    RegInit(VecInit(Seq.fill(scanChunkCount)(0.U(scanChunkBits.W))))
  val scan_outstanding = RegInit(0.U(log2Ceil(threadSupport + 1).W))
  val scan_base_valid = RegInit(false.B)
  val scan_base_thread = RegInit(0.U(threadIdLength.W))
  val scan_winner_valid = RegInit(false.B)
  val scan_winner_thread = RegInit(0.U(threadIdLength.W))
  val scan_winner_meta = RegInit(zeroSchedMeta)
  val scan_reason = RegInit(0.U(3.W))
  // Raw winners are re-read before they can update staged or redirect a load.
  val scan_epoch = RegInit(0.U(8.W))
  val scan_revalidate_pending = RegInit(false.B)
  val scan_revalidate_outstanding = RegInit(false.B)
  val scan_revalidate_thread = RegInit(0.U(threadIdLength.W))
  val scan_revalidate_snapshot = RegInit(zeroSchedMeta)
  val deferred_rank_valid = RegInit(false.B)
  val deferred_rank_thread = RegInit(0.U(threadIdLength.W))
  val deferred_rank_meta = RegInit(zeroSchedMeta)
  val staged_recheck_pending = RegInit(false.B)

  when (io.slot_dirty_set.valid) {
    slot_dirty(io.slot_dirty_set.bits) := true.B
  }
  when (io.save_done.valid) {
    slot_dirty(io.save_done.bits.slot) := false.B
  }

  val resident_mask = thread_resident
  val pinned_bitmap = UIntToOH(1.U(threadIdLength.W), threadSupport)
  val tpt_ready_bitmap = Cat(io.tpt_ready_chunks.reverse)
  val unloaded_runnable_bitmap = tpt_ready_bitmap & ~resident_mask
  private def scanCandidates(resident: UInt, excludeValid: Bool, excludeThread: UInt): UInt =
    (tpt_ready_bitmap & ~resident) &
      ~Mux(excludeValid, threadOH(excludeThread), 0.U(threadSupport.W))
  private def maskChunks(mask: UInt): Vec[UInt] =
    VecInit((0 until scanChunkCount).map { c =>
      mask((c + 1) * scanChunkBits - 1, c * scanChunkBits)
    })
  val staged_ready = staged_valid_reg && schedReadyMeta(staged_meta_reg) &&
    !bit(resident_mask, staged_thread_reg)

  io.staged_valid := staged_valid_reg
  io.staged_thread := staged_thread_reg
  io.rank_change.ready := true.B

  val changed_thread = io.rank_change.bits.thread
  val changed_meta = io.rank_change.bits.meta
  val changed_rerank = io.rank_change.bits.rerank
  val rank_change_fire = io.rank_change.fire
  val replacement_busy = state =/= s_idle
  val changed_is_resident = bit(resident_mask, changed_thread)
  val changed_is_staged = staged_valid_reg && (changed_thread === staged_thread_reg)
  val changed_is_deferred = deferred_rank_valid && (changed_thread === deferred_rank_thread)
  val changed_is_replacement_thread = replacement_busy &&
    ((changed_thread === rep_staged) || (changed_thread === rep_victim))
  val changed_ready = schedReadyMeta(changed_meta) && !changed_is_resident &&
    !changed_is_replacement_thread
  val changed_beats_staged = !staged_ready ||
    rankHigherMeta(changed_thread, changed_meta, staged_thread_reg, staged_meta_reg)
  val changed_ordinary_unloaded = rank_change_fire && changed_rerank &&
    !changed_is_staged && !changed_is_deferred && changed_ready

  val deferred_self_update = rank_change_fire && changed_is_deferred
  val deferred_rank_valid_eff =
    Mux(deferred_self_update, schedReadyMeta(changed_meta), deferred_rank_valid)
  val deferred_rank_meta_eff =
    muxMeta(deferred_self_update, changed_meta, deferred_rank_meta)
  val deferred_rank_ready = deferred_rank_valid_eff && schedReadyMeta(deferred_rank_meta_eff) &&
    !bit(resident_mask, deferred_rank_thread) &&
    !(staged_valid_reg && (deferred_rank_thread === staged_thread_reg)) &&
    !(replacement_busy &&
      ((deferred_rank_thread === rep_staged) || (deferred_rank_thread === rep_victim)))
  val changed_beats_deferred = !deferred_rank_ready ||
    rankHigherMeta(changed_thread, changed_meta, deferred_rank_thread, deferred_rank_meta)
  val defer_rank_change = changed_ordinary_unloaded && replacement_busy && changed_beats_deferred
  val pending_deferred_valid = deferred_rank_valid_eff || defer_rank_change
  val pending_deferred_thread = Mux(defer_rank_change, changed_thread, deferred_rank_thread)
  val pending_deferred_meta = muxMeta(defer_rank_change, changed_meta, deferred_rank_meta_eff)

  val direct_stage_rank_change = changed_ordinary_unloaded &&
    (state === s_idle) && changed_beats_staged
  val ordinary_unloaded_stage_swap = direct_stage_rank_change
  val scan_winner_revalidation_inflight =
    scan_revalidate_pending || scan_revalidate_outstanding
  val scanner_busy = scan_active || scan_winner_revalidation_inflight
  val staged_refill_from_rank_change = rank_change_fire && changed_rerank && changed_is_staged &&
    (state === s_idle) && !scanner_busy
  val changed_staged_ready = schedReadyMeta(changed_meta) &&
    !bit(resident_mask, changed_thread)
  val staged_recheck_set = rank_change_fire && changed_rerank && changed_is_staged &&
    !staged_refill_from_rank_change
  val staged_recheck_scan_start = staged_recheck_pending && (state === s_idle) && !scanner_busy

  val load_done_commit = (state === s_load_wait) &&
    io.load_done.valid && (io.load_done.bits.slot === rep_slot) &&
    (io.load_done.bits.thread === rep_staged)
  val slot_load_commit_done = (state === s_load_commit_wait) &&
    io.slot_load_commit.valid &&
    (io.slot_load_commit.bits.slot === rep_slot) &&
    (io.slot_load_commit.bits.thread === rep_staged)
  val rep_staged_meta_eff = muxMeta(
    rank_change_fire && replacement_busy && (changed_thread === rep_staged),
    changed_meta, rep_staged_meta)
  val rep_victim_meta_eff = muxMeta(
    rank_change_fire && replacement_busy && (changed_thread === rep_victim),
    changed_meta, rep_victim_meta)
  val resident_mask_after_load =
    (resident_mask | threadOH(rep_staged)) & ~threadOH(rep_victim)
  val victim_ready_after_load = schedReadyMeta(rep_victim_meta_eff) &&
    !bit(resident_mask_after_load, rep_victim)
  val post_load_deferred_ready = pending_deferred_valid &&
    schedReadyMeta(pending_deferred_meta) &&
    !bit(resident_mask_after_load, pending_deferred_thread) &&
    (pending_deferred_thread =/= rep_staged) &&
    (pending_deferred_thread =/= rep_victim)
  val post_load_deferred_beats = post_load_deferred_ready &&
    (!victim_ready_after_load ||
      rankHigherMeta(pending_deferred_thread, pending_deferred_meta, rep_victim, rep_victim_meta_eff))
  val post_load_staged_valid = victim_ready_after_load || post_load_deferred_ready
  val post_load_staged_thread =
    Mux(post_load_deferred_beats, pending_deferred_thread, rep_victim)
  val post_load_staged_meta =
    muxMeta(post_load_deferred_beats, pending_deferred_meta, rep_victim_meta_eff)

  val scan_chunk_nonempty = VecInit(scan_candidate_chunks.map(_.orR)).asUInt
  val scan_issue_chunk =
    if (scanChunkCount == 1) 0.U else PriorityEncoder(scan_chunk_nonempty)
  val scan_issue_offset = PriorityEncoder(scan_candidate_chunks(scan_issue_chunk))
  val scan_issue_thread =
    (scan_issue_chunk * scanChunkBits.U + scan_issue_offset)(threadIdLength - 1, 0)
  val tpt_req_is_revalidate = scan_revalidate_pending
  io.tpt_read_req.valid :=
    scan_revalidate_pending || (scan_active && scan_chunk_nonempty.orR)
  io.tpt_read_req.bits.kind := Mux(tpt_req_is_revalidate,
    ThreadTptReadKind.revalidate.U, ThreadTptReadKind.scan.U)
  io.tpt_read_req.bits.thread :=
    Mux(tpt_req_is_revalidate, scan_revalidate_thread, scan_issue_thread)
  io.tpt_read_req.bits.epoch := scan_epoch
  val scan_req_fire = io.tpt_read_req.fire && !tpt_req_is_revalidate
  val scan_revalidate_req_fire = io.tpt_read_req.fire && tpt_req_is_revalidate
  val tpt_resp_req = io.tpt_read_resp.bits.req
  val tpt_resp_epoch_matches = tpt_resp_req.epoch === scan_epoch
  val tpt_resp_is_scan = tpt_resp_req.kind === ThreadTptReadKind.scan.U
  val tpt_resp_is_revalidate = tpt_resp_req.kind === ThreadTptReadKind.revalidate.U
  val scan_resp_valid = scan_active && io.tpt_read_resp.valid &&
    tpt_resp_is_scan && tpt_resp_epoch_matches
  val scan_revalidate_resp = scan_revalidate_outstanding &&
    io.tpt_read_resp.valid && tpt_resp_is_revalidate &&
    tpt_resp_epoch_matches &&
    (tpt_resp_req.thread === scan_revalidate_thread)
  val scan_resp_thread = tpt_resp_req.thread
  val scan_resp_meta = io.tpt_read_resp.bits.meta
  val scan_thread_ready = scan_resp_valid && schedReadyMeta(scan_resp_meta) &&
    !bit(resident_mask, scan_resp_thread) &&
    !(scan_base_valid && (scan_resp_thread === scan_base_thread))
  val scan_take = scan_thread_ready &&
    (!scan_winner_valid ||
      rankHigherMeta(scan_resp_thread, scan_resp_meta, scan_winner_thread, scan_winner_meta))
  val scan_next_winner_valid = scan_winner_valid || scan_thread_ready
  val scan_next_winner_thread = Mux(scan_take, scan_resp_thread, scan_winner_thread)
  val scan_next_winner_meta = muxMeta(scan_take, scan_resp_meta, scan_winner_meta)
  val scan_candidate_chunks_next = WireDefault(scan_candidate_chunks)
  when (scan_req_fire) {
    scan_candidate_chunks_next(scan_issue_chunk) :=
      scan_candidate_chunks(scan_issue_chunk) &
        ~UIntToOH(scan_issue_offset, scanChunkBits)
  }
  val scan_candidates_empty_next =
    !VecInit(scan_candidate_chunks_next.map(_.orR)).asUInt.orR
  val scan_outstanding_next = WireDefault(scan_outstanding)
  when (scan_req_fire && !scan_resp_valid) {
    scan_outstanding_next := scan_outstanding + 1.U
  }.elsewhen (!scan_req_fire && scan_resp_valid) {
    scan_outstanding_next := scan_outstanding - 1.U
  }
  val scan_done = scan_active && scan_candidates_empty_next &&
    (scan_outstanding_next === 0.U)
  when (scan_resp_valid) {
    threadletAssert(scan_outstanding =/= 0.U,
      "TPT scanner response must match an outstanding request")
  }
  when (io.tpt_read_resp.valid && tpt_resp_is_revalidate && tpt_resp_epoch_matches) {
    threadletAssert(scan_revalidate_outstanding,
      "TPT scanner revalidation response must match an outstanding request")
    threadletAssert(tpt_resp_req.thread === scan_revalidate_thread,
      "TPT scanner revalidation response must match the raw winner")
  }

  val scan_revalidate_meta_unchanged =
    scan_resp_meta.asUInt === scan_revalidate_snapshot.asUInt
  val scan_result_valid = scan_revalidate_resp &&
    scan_revalidate_meta_unchanged &&
    schedReadyMeta(scan_resp_meta) &&
    !bit(resident_mask, scan_resp_thread)
  val scan_revalidate_restart = scan_revalidate_resp && !scan_result_valid

  val staged_self_update = rank_change_fire && changed_is_staged
  val staged_current_meta = muxMeta(staged_self_update, changed_meta, staged_meta_reg)
  val scan_current_ready = staged_valid_reg && schedReadyMeta(staged_current_meta) &&
    !bit(resident_mask, staged_thread_reg)
  val staged_after_rank_valid = scan_current_ready || ordinary_unloaded_stage_swap
  val staged_after_rank_thread =
    Mux(ordinary_unloaded_stage_swap, changed_thread, staged_thread_reg)
  val staged_after_rank_meta =
    muxMeta(ordinary_unloaded_stage_swap, changed_meta, staged_current_meta)
  val scan_result_beats_staged = scan_result_valid &&
    (!staged_after_rank_valid ||
      (scan_resp_thread === staged_after_rank_thread) ||
      rankHigherMeta(scan_resp_thread, scan_resp_meta,
        staged_after_rank_thread, staged_after_rank_meta))
  val scan_final_valid = staged_after_rank_valid || scan_result_valid
  val scan_final_thread =
    Mux(scan_result_beats_staged, scan_resp_thread, staged_after_rank_thread)
  val scan_final_meta =
    muxMeta(scan_result_beats_staged, scan_resp_meta, staged_after_rank_meta)
  val scan_idle_commit = (state === s_idle) && scan_result_valid

  val scan_result_for_replacement = scan_result_valid && replacement_busy &&
    (scan_resp_thread =/= rep_staged) &&
    (scan_resp_thread =/= rep_victim)
  val pending_deferred_merge_ready = pending_deferred_valid &&
    schedReadyMeta(pending_deferred_meta) &&
    !bit(resident_mask, pending_deferred_thread) &&
    !(replacement_busy &&
      ((pending_deferred_thread === rep_staged) ||
       (pending_deferred_thread === rep_victim)))
  val scan_candidate_matches_deferred = pending_deferred_merge_ready &&
    (scan_resp_thread === pending_deferred_thread)
  val scan_candidate_beats_deferred = scan_result_for_replacement &&
    (!pending_deferred_merge_ready ||
      scan_candidate_matches_deferred ||
      rankHigherMeta(scan_resp_thread, scan_resp_meta,
        pending_deferred_thread, pending_deferred_meta))

  val slot_locked = VecInit((0 until threadSlotCount).map { s =>
    slot_replacing(s) || io.saver_busy_slots(s)
  }).asUInt
  io.slot_locked := slot_locked
  io.slot_sched_blocked := slot_locked
  io.slot_ctx_write_blocked := VecInit((0 until threadSlotCount).map { s =>
    val draining_this_slot = (state === s_drain) && (rep_slot === s.U(threadSlotIdLength.W))
    bit(io.saver_busy_slots, s.U(threadSlotIdLength.W)) ||
      (slot_replacing(s) && !draining_this_slot)
  }).asUInt
  io.slot_thread := slot_thread

  val victim_eligible = VecInit((0 until threadSlotCount).map { s =>
    val t = slot_thread(s)
    !slot_locked(s) &&
      (t =/= io.current_thread) &&
      !bit(pinned_bitmap, t)
  }).asUInt
  var victim_found = false.B
  var victim_slot = 0.U(threadSlotIdLength.W)
  for (s <- 0 until threadSlotCount) {
    val cand = slot_thread(s)
    val cur = slot_thread(victim_slot)
    val candMeta = io.slot_meta(s)
    val curMeta = io.slot_meta(victim_slot)
    val take = victim_eligible(s) && (!victim_found ||
      rankHigherMeta(cur, curMeta, cand, candMeta))
    victim_slot = Mux(take, s.U(threadSlotIdLength.W), victim_slot)
    victim_found = victim_found || victim_eligible(s)
  }
  val victim_thread = slot_thread(victim_slot)
  val victim_meta = io.slot_meta(victim_slot)
  val replace_wanted = staged_ready && victim_found &&
    rankHigherMeta(staged_thread_reg, staged_meta_reg, victim_thread, victim_meta)
  val start_replacement = (state === s_idle) && replace_wanted && io.can_start &&
    !io.metadata_pending &&
    !scan_winner_revalidation_inflight &&
    !direct_stage_rank_change && !io.rank_change.valid &&
    !staged_refill_from_rank_change && !staged_recheck_scan_start
  val rep_staged_ready_for_load = schedReadyMeta(rep_staged_meta_eff) && !bit(resident_mask, rep_staged)
  val rep_staged_beats_victim =
    rankHigherMeta(rep_staged, rep_staged_meta_eff, rep_victim, rep_victim_meta_eff)
  val pending_deferred_ready_for_load = pending_deferred_valid &&
    schedReadyMeta(pending_deferred_meta) &&
    !bit(resident_mask, pending_deferred_thread) &&
    (pending_deferred_thread =/= rep_staged) &&
    (pending_deferred_thread =/= rep_victim)
  val use_deferred_for_load = pending_deferred_ready_for_load &&
    rankHigherMeta(pending_deferred_thread, pending_deferred_meta, rep_victim, rep_victim_meta_eff) &&
    (!rep_staged_ready_for_load ||
      rankHigherMeta(pending_deferred_thread, pending_deferred_meta, rep_staged, rep_staged_meta_eff))
  val redirect_deferred_for_load = (state === s_load_wait) &&
    bit(io.saver_busy_slots, rep_slot) &&
    !io.metadata_pending &&
    !scan_winner_revalidation_inflight &&
    use_deferred_for_load
  val load_target_thread = Mux(use_deferred_for_load, pending_deferred_thread, rep_staged)
  val load_target_meta = muxMeta(use_deferred_for_load, pending_deferred_meta, rep_staged_meta_eff)
  val load_target_ready = schedReadyMeta(load_target_meta) && !bit(resident_mask, load_target_thread)
  val load_target_ok = load_target_ready &&
    rankHigherMeta(load_target_thread, load_target_meta, rep_victim, rep_victim_meta_eff)

  val cancel_deferred_ready = pending_deferred_valid &&
    schedReadyMeta(pending_deferred_meta) &&
    !bit(resident_mask, pending_deferred_thread) &&
    (pending_deferred_thread =/= rep_victim)
  val cancel_use_deferred = cancel_deferred_ready &&
    (!rep_staged_ready_for_load ||
      rankHigherMeta(pending_deferred_thread, pending_deferred_meta, rep_staged, rep_staged_meta_eff))
  val cancel_base_valid = rep_staged_ready_for_load || cancel_deferred_ready
  val cancel_base_thread = Mux(cancel_use_deferred, pending_deferred_thread, rep_staged)
  val cancel_base_meta = muxMeta(cancel_use_deferred, pending_deferred_meta, rep_staged_meta_eff)

  io.save_start.valid := false.B
  io.save_start.bits.thread := rep_victim
  io.save_start.bits.slot := rep_slot
  io.load_start.valid := false.B
  io.load_start.bits.thread := load_target_thread
  io.load_start.bits.slot := rep_slot
  io.load_redirect.valid := redirect_deferred_for_load
  io.load_redirect.bits.thread := pending_deferred_thread
  io.load_redirect.bits.slot := rep_slot
  io.slot_load_request.valid :=
    (state === s_load_metadata_wait) && !io.metadata_pending
  io.slot_load_request.bits.thread := rep_staged
  io.slot_load_request.bits.slot := rep_slot
  io.kill_thread.valid := false.B
  io.kill_thread.bits := rep_victim
  val dbg_stage_swap_valid = WireDefault(false.B)

  when (scan_revalidate_req_fire) {
    scan_revalidate_pending := false.B
    scan_revalidate_outstanding := true.B
  }
  when (scan_revalidate_resp) {
    scan_revalidate_outstanding := false.B
  }

  when (rank_change_fire && changed_is_staged) {
    staged_meta_reg := changed_meta
  }
  when (rank_change_fire && changed_is_deferred) {
    deferred_rank_valid := schedReadyMeta(changed_meta)
    deferred_rank_meta := changed_meta
  }
  when (rank_change_fire && replacement_busy && (changed_thread === rep_staged)) {
    rep_staged_meta := changed_meta
  }
  when (rank_change_fire && replacement_busy && (changed_thread === rep_victim)) {
    rep_victim_meta := changed_meta
  }
  when (defer_rank_change) {
    deferred_rank_valid := true.B
    deferred_rank_thread := changed_thread
    deferred_rank_meta := changed_meta
  }
  when (staged_recheck_set) {
    staged_recheck_pending := true.B
  }

  when (scan_active) {
    scan_candidate_chunks := scan_candidate_chunks_next
    scan_outstanding := scan_outstanding_next
    scan_winner_valid := scan_next_winner_valid
    scan_winner_thread := scan_next_winner_thread
    scan_winner_meta := scan_next_winner_meta
    when (scan_done) {
      scan_active := false.B
      when (scan_next_winner_valid) {
        scan_revalidate_pending := true.B
        scan_revalidate_thread := scan_next_winner_thread
        scan_revalidate_snapshot := scan_next_winner_meta
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[TCM][scan_revalidate] winner=%d r=%d\n",
            scan_next_winner_thread, scan_reason))
        }
      }.otherwise {
        when (state === s_idle) {
          staged_valid_reg := staged_after_rank_valid
          staged_thread_reg := staged_after_rank_thread
          staged_meta_reg := staged_after_rank_meta
          staged_recheck_pending := false.B
          threadletPrintf {
            midas.targetutils.SynthesizePrintf(printf(
              "[TCM][stage_refill] cur=%d win_v=0 new_v=%d new=%d r=%d\n",
              staged_thread_reg, staged_after_rank_valid,
              staged_after_rank_thread, scan_reason))
          }
        }
      }
    }
  }

  when (scan_revalidate_restart) {
    scan_active := true.B
    scan_candidate_chunks :=
      maskChunks(scanCandidates(resident_mask, scan_current_ready, staged_thread_reg))
    scan_outstanding := 0.U
    scan_base_valid := scan_current_ready
    scan_base_thread := staged_thread_reg
    scan_winner_valid := false.B
    scan_winner_thread := 0.U
    scan_winner_meta := zeroSchedMeta
    scan_reason := 5.U
    scan_revalidate_pending := false.B
    scan_revalidate_outstanding := false.B
    scan_epoch := scan_epoch + 1.U
    threadletPrintf {
      midas.targetutils.SynthesizePrintf(printf(
        "[TCM][scan_retry] winner=%d ready=%d stable=%d\n",
        scan_resp_thread, schedReadyMeta(scan_resp_meta),
        scan_revalidate_meta_unchanged))
    }
  }

  when (scan_candidate_beats_deferred) {
    deferred_rank_valid := true.B
    deferred_rank_thread := scan_resp_thread
    deferred_rank_meta := scan_resp_meta
  }

  switch (state) {
    is (s_idle) {
      when (scan_idle_commit) {
        staged_valid_reg := scan_final_valid
        staged_thread_reg := scan_final_thread
        staged_meta_reg := scan_final_meta
        staged_recheck_pending := false.B
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[TCM][stage_refill] cur=%d win_v=1 win=%d new=%d r=%d\n",
            staged_thread_reg, scan_resp_thread, scan_final_thread, scan_reason))
        }
      }.elsewhen (ordinary_unloaded_stage_swap) {
        staged_valid_reg := true.B
        staged_thread_reg := changed_thread
        staged_meta_reg := changed_meta
        dbg_stage_swap_valid := true.B
      }.elsewhen (staged_refill_from_rank_change) {
        scan_active := true.B
        scan_candidate_chunks := maskChunks(unloaded_runnable_bitmap &
          ~Mux(changed_staged_ready, threadOH(staged_thread_reg), 0.U(threadSupport.W)))
        scan_outstanding := 0.U
        scan_base_valid := changed_staged_ready
        scan_base_thread := staged_thread_reg
        scan_winner_valid := false.B
        scan_winner_thread := 0.U
        scan_winner_meta := zeroSchedMeta
        scan_reason := 3.U
        scan_revalidate_pending := false.B
        scan_revalidate_outstanding := false.B
        scan_epoch := scan_epoch + 1.U
      }.elsewhen (staged_recheck_scan_start) {
        scan_active := true.B
        scan_candidate_chunks := maskChunks(unloaded_runnable_bitmap &
          ~Mux(staged_ready, threadOH(staged_thread_reg), 0.U(threadSupport.W)))
        scan_outstanding := 0.U
        scan_base_valid := staged_ready
        scan_base_thread := staged_thread_reg
        scan_winner_valid := false.B
        scan_winner_thread := 0.U
        scan_winner_meta := zeroSchedMeta
        scan_reason := 3.U
        staged_recheck_pending := false.B
        scan_revalidate_pending := false.B
        scan_revalidate_outstanding := false.B
        scan_epoch := scan_epoch + 1.U
      }.elsewhen (start_replacement) {
        rep_slot := victim_slot
        rep_victim := victim_thread
        rep_staged := staged_thread_reg
        rep_victim_meta := victim_meta
        rep_staged_meta := staged_meta_reg
        slot_replacing(victim_slot) := true.B
        io.kill_thread.valid := true.B
        io.kill_thread.bits := victim_thread
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[TCM][swap_start] slot=%d victim=%d staged=%d\n",
            victim_slot, victim_thread, staged_thread_reg))
        }
        state := s_drain
      }
    }

    is (s_drain) {
      when (!bit(io.slot_drain_busy, rep_slot) && !bit(io.saver_busy_slots, rep_slot) &&
          io.can_start && !io.ctx_write_busy && !io.metadata_pending) {
        when (slot_dirty(rep_slot) && rep_victim_meta.valid) {
          io.save_start.valid := true.B
          state := s_save_wait
        }.otherwise {
          state := s_load_start
        }
      }
    }

    is (s_save_wait) {
      when (io.save_done.valid && (io.save_done.bits.slot === rep_slot)) {
        state := s_load_start
      }
    }

    is (s_load_start) {
      when (!bit(io.saver_busy_slots, rep_slot) && io.can_start &&
          !scan_winner_revalidation_inflight && !io.metadata_pending) {
        when (load_target_ok) {
          io.load_start.valid := true.B
          rep_staged := load_target_thread
          rep_staged_meta := load_target_meta
          when (use_deferred_for_load) {
            deferred_rank_valid := false.B
            threadletPrintf {
              midas.targetutils.SynthesizePrintf(printf(
                "[TCM][defer_stage] t=%d mode=%d\n",
                pending_deferred_thread, 0.U))
            }
          }
          state := s_load_wait
        }.otherwise {
          slot_replacing(rep_slot) := false.B
          staged_valid_reg := cancel_base_valid
          staged_thread_reg := cancel_base_thread
          staged_meta_reg := cancel_base_meta
          deferred_rank_valid := false.B
          scan_active := true.B
          scan_candidate_chunks :=
            maskChunks(scanCandidates(resident_mask, cancel_base_valid, cancel_base_thread))
          scan_outstanding := 0.U
          scan_base_valid := cancel_base_valid
          scan_base_thread := cancel_base_thread
          scan_winner_valid := false.B
          scan_winner_thread := 0.U
          scan_winner_meta := zeroSchedMeta
          scan_reason := 4.U
          scan_revalidate_pending := false.B
          scan_revalidate_outstanding := false.B
          scan_epoch := scan_epoch + 1.U
          threadletPrintf {
            midas.targetutils.SynthesizePrintf(printf(
              "[TCM][swap_cancel] slot=%d victim=%d staged=%d\n",
              rep_slot, rep_victim, rep_staged))
          }
          state := s_idle
        }
      }
    }

    is (s_load_wait) {
      when (load_done_commit) {
        state := s_load_metadata_wait
      }.elsewhen (io.load_redirect.fire) {
        rep_staged := pending_deferred_thread
        rep_staged_meta := pending_deferred_meta
        deferred_rank_valid := false.B
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[TCM][load_redirect] slot=%d old=%d new=%d\n",
            rep_slot, rep_staged, pending_deferred_thread))
        }
      }
    }

    is (s_load_metadata_wait) {
      when (io.slot_load_request.valid) {
        state := s_load_commit_wait
      }
    }

    is (s_load_commit_wait) {
      when (slot_load_commit_done) {
        thread_resident := resident_mask_after_load
        slot_thread(rep_slot) := rep_staged
        slot_dirty(rep_slot) := false.B
        slot_replacing(rep_slot) := false.B
        staged_valid_reg := post_load_staged_valid
        staged_thread_reg := post_load_staged_thread
        staged_meta_reg := post_load_staged_meta
        deferred_rank_valid := false.B
        scan_active := true.B
        scan_candidate_chunks :=
          maskChunks(scanCandidates(
            resident_mask_after_load, post_load_staged_valid, post_load_staged_thread))
        scan_outstanding := 0.U
        scan_base_valid := post_load_staged_valid
        scan_base_thread := post_load_staged_thread
        scan_winner_valid := false.B
        scan_winner_thread := 0.U
        scan_winner_meta := zeroSchedMeta
        scan_reason := 2.U
        scan_revalidate_pending := false.B
        scan_revalidate_outstanding := false.B
        scan_epoch := scan_epoch + 1.U
        staged_recheck_pending := false.B
        threadletPrintf {
          midas.targetutils.SynthesizePrintf(printf(
            "[TCM][swap_done] slot=%d old=%d new=%d\n",
            rep_slot, rep_victim, rep_staged))
        }
        when (post_load_deferred_beats) {
          threadletPrintf {
            midas.targetutils.SynthesizePrintf(printf(
              "[TCM][defer_stage] t=%d mode=%d\n",
              pending_deferred_thread, 1.U))
          }
        }
        state := s_idle
      }
    }
  }

  when (dbg_stage_swap_valid) {
    threadletPrintf {
      midas.targetutils.SynthesizePrintf(printf(
        "[TCM][stage_swap] old_v=%d old=%d new=%d\n",
        staged_valid_reg, staged_thread_reg, changed_thread))
    }
  }

  threadletAssert(!(state =/= s_idle && !slot_replacing(rep_slot)),
    "replacement slot must stay locked until context load commits")
  threadletAssert(!(state =/= s_idle && (ordinary_unloaded_stage_swap || staged_refill_from_rank_change)),
    "staged metadata maintenance must only run while replacement FSM is idle")
  when (start_replacement) {
    threadletAssert(staged_ready, "replacement must start from a valid runnable nonresident staged threadlet")
  }
  when (io.load_start.valid || io.slot_load_request.valid || io.load_redirect.valid) {
    threadletAssert(!io.metadata_pending,
      "replacement must not consume metadata while an older metadata snapshot is pending")
  }
}
