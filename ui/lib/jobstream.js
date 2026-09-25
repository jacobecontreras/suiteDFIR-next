// @ts-check
/**
 * The event stream of the one active job (ARCHITECTURE.md D25), kept outside the screens so the
 * Run and Acquire screens can be left and reopened without losing the log (tests/ui/jobstream.test.js).
 *
 * - A job started in this window streams into the hub from the `onEvent` channel passed to
 *   `run_start` / `acq_start` (`begin`).
 * - After a window reload the channel is gone; `attach` subscribes again with `job_attach`, which
 *   returns the core's log backlog (the last 2,000 lines) and replaces the previous subscriber.
 *
 * Log lines are kept in one growing array (the log view renders only the rows in view). Past
 * `MAX_LOG_LINES` the oldest lines are dropped in chunks and counted; the complete log is always in
 * the run folder (`report/_HTML/_Script_Logs/Screen_Output.html`) or the acquisition's stdout log.
 */

/** @typedef {import("../types").AcqEvent} AcqEvent */
/** @typedef {import("../types").ActiveJob} ActiveJob */
/** @typedef {import("../types").DevicePromptKind} DevicePromptKind */
/** @typedef {import("../types").JobKind} JobKind */
/** @typedef {import("../types").RunEvent} RunEvent */
/** @typedef {import("./context").AppState} AppState */
/** @typedef {Extract<RunEvent, { type: "finished" }>} RunFinished */
/** @typedef {Extract<AcqEvent, { type: "finished" }>} AcqFinished */

/** Lines kept in memory for the log view; ~40 MB of typical log text. */
export const MAX_LOG_LINES = 200_000;
/** Dropped at once when the cap is exceeded, so dropping is rare. */
export const LOG_DROP_CHUNK = 20_000;

export const RUN_PHASES = /** @type {const} */ (["preparing", "running", "hashing_input", "analyzing", "sealing_report", "finalizing"]);
export const ACQ_PHASES = /** @type {const} */ ([
  "preparing",
  "enabling_encryption",
  "backing_up",
  "restoring_encryption",
  "validating",
  "sealing",
  "finalizing",
]);

/**
 * @typedef {object} LogBuffer
 * @property {string[]} lines
 * @property {number} dropped Lines dropped from the start (so line N is `lines[N - 1 - dropped]`).
 */

/**
 * @typedef {object} JobStream
 * @property {JobKind} kind
 * @property {string | null} id `null` until `run_start` / `acq_start` returns.
 * @property {string} casePath
 * @property {boolean} live Events are flowing (ends with `finished`).
 * @property {boolean} partial Attached after a reload: the phases before the attach were not seen.
 * @property {boolean} acknowledged The examiner dismissed the result (Acquire: "New acquisition").
 * @property {string | null} phase
 * @property {string[]} phases Phases seen, in order.
 * @property {LogBuffer} log
 * @property {{ stdout: string[] | null, stderr: string[] | null }} stdio Run only.
 * @property {{ done: number, total: number } | null} hash Run only: input hashing progress.
 * @property {{ done: number, total: number | null } | null} seal Report or backup sealing progress.
 * @property {number | null} percent Acquisition only: overall backup percent.
 * @property {{ kind: DevicePromptKind, text: string } | null} prompt Acquisition only: the device waits for the examiner.
 * @property {RunFinished | AcqFinished | null} finished
 * @property {number} version Bumped on every change.
 */

/**
 * @param {JobKind} kind
 * @param {string | null} id
 * @param {string} casePath
 * @returns {JobStream}
 */
export function newStream(kind, id, casePath) {
  return {
    kind,
    id,
    casePath,
    live: true,
    partial: false,
    acknowledged: false,
    phase: null,
    phases: [],
    log: { lines: [], dropped: 0 },
    stdio: { stdout: null, stderr: null },
    hash: null,
    seal: null,
    percent: null,
    prompt: null,
    finished: null,
    version: 0,
  };
}

/**
 * Appends lines, dropping the oldest `dropChunk` (or more) once the buffer exceeds `max`.
 * @param {LogBuffer} log
 * @param {readonly string[]} lines
 * @param {number} [max]
 * @param {number} [dropChunk]
 */
export function appendLog(log, lines, max = MAX_LOG_LINES, dropChunk = LOG_DROP_CHUNK) {
  for (const line of lines) log.lines.push(line);
  if (log.lines.length > max) {
    const n = Math.min(log.lines.length, log.lines.length - max + dropChunk);
    log.lines.splice(0, n);
    log.dropped += n;
  }
}

/**
 * Applies one `RunEvent` or `AcqEvent` to a stream (in place).
 * @param {JobStream} s
 * @param {RunEvent | AcqEvent} event
 */
export function applyEvent(s, event) {
  switch (event.type) {
    case "phase":
      s.phase = event.phase;
      if (s.phases[s.phases.length - 1] !== event.phase) s.phases.push(event.phase);
      // A prompt belongs to the step that asked for it.
      s.prompt = null;
      break;
    case "log":
      appendLog(s.log, event.lines);
      break;
    case "stdio_tail":
      s.stdio = { ...s.stdio, [event.stream]: [...event.lines] };
      break;
    case "hash_progress":
      s.hash = { done: event.bytes_done, total: event.bytes_total };
      break;
    case "seal_progress":
      s.seal = { done: event.files_done, total: event.files_total };
      break;
    case "progress":
      s.percent = event.percent;
      // Progress after a passcode prompt means the passcode was entered.
      s.prompt = null;
      break;
    case "device_prompt":
      s.prompt = { kind: event.kind, text: event.text };
      break;
    case "finished":
      s.finished = event;
      s.live = false;
      s.prompt = null;
      break;
  }
  s.version += 1;
}

/**
 * @typedef {"done" | "current" | "skipped" | "pending"} StepState
 */

/**
 * The state of each step of an ordered phase list, for the phase stepper.
 * - Steps seen before the current one are done; unseen ones were skipped (e.g. no report to seal),
 *   unless the stream attached mid-way (`partial`), when earlier steps count as done.
 * - Optional steps (input hashing, the encryption steps) are listed only if seen or current (after
 *   a reload it is unknown whether an earlier optional step ran, so it is left out).
 * - Once finished, there is no current step.
 * @param {readonly string[]} order
 * @param {{ phases: readonly string[], phase: string | null, finished: boolean, partial: boolean }} s
 * @param {ReadonlySet<string>} [optional]
 * @returns {{ phase: string, state: StepState }[]}
 */
export function stepStates(order, s, optional = new Set()) {
  const seen = new Set(s.phases);
  const at = s.phase === null ? -1 : order.indexOf(s.phase);
  /** @type {{ phase: string, state: StepState }[]} */
  const out = [];
  order.forEach((phase, i) => {
    /** @type {StepState} */
    let state;
    if (s.finished) state = seen.has(phase) || (s.partial && i <= at) ? "done" : "skipped";
    else if (i < at) state = seen.has(phase) || s.partial ? "done" : "skipped";
    else if (i === at) state = "current";
    else state = "pending";
    if (optional.has(phase) && !seen.has(phase) && state !== "current") return;
    out.push({ phase, state });
  });
  return out;
}

/**
 * @param {ActiveJob | null | undefined} job
 * @returns {string | null}
 */
export function activeJobId(job) {
  if (!job) return null;
  return job.kind === "run" ? job.run_id : job.acq_id;
}

/**
 * @typedef {object} Beginning
 * @property {(event: RunEvent | AcqEvent) => void} onEvent The channel callback for `run_start` / `acq_start`.
 * @property {(id: string) => void} bind Call with the returned `run_id` / `acq_id`.
 * @property {() => void} abandon Call when the start command failed.
 */

/**
 * @typedef {object} JobStreams
 * @property {() => JobStream | null} current The stream of the active job, or of the last one (kept for its result).
 * @property {(kind: JobKind, id: string) => JobStream | null} find
 * @property {(kind: JobKind, casePath: string) => Beginning} begin
 * @property {(api: AttachApi, kind: JobKind, id: string, casePath: string) => Promise<JobStream>} attach
 *   Subscribes again after a reload (`job_attach`); returns the existing stream when it is still
 *   fed or already finished. Rejects with the command's `AppError` (e.g. the job already ended).
 * @property {(fn: (s: JobStream) => void) => () => void} subscribe Called on every change.
 */

/**
 * @typedef {{ job_attach: (req: import("../types").JobAttachRequest, onEvent: (e: RunEvent | AcqEvent) => void) => Promise<import("../types").JobBacklog> }} AttachApi
 */

/**
 * @param {import("./store.js").Store<AppState>} store Keeps `activeJob.phase` current and clears it on `finished`.
 * @param {{ onFinished?: (s: JobStream) => void }} [hooks]
 * @returns {JobStreams}
 */
export function createJobStreams(store, hooks = {}) {
  /** @type {JobStream | null} */
  let current = null;
  /** @type {Set<(s: JobStream) => void>} */
  const subscribers = new Set();

  /** @param {JobStream} s */
  const notify = (s) => {
    for (const fn of [...subscribers]) fn(s);
  };

  /**
   * @param {JobStream} s
   * @param {RunEvent | AcqEvent} event
   */
  function deliver(s, event) {
    applyEvent(s, event);
    syncActiveJob(s, event);
    if (event.type === "finished") hooks.onFinished?.(s);
    notify(s);
  }

  /**
   * @param {JobStream} s
   * @param {RunEvent | AcqEvent} event
   */
  function syncActiveJob(s, event) {
    const job = store.get().activeJob;
    if (!job || activeJobId(job) !== s.id) return;
    if (event.type === "finished") {
      store.set({ activeJob: null });
    } else if (event.type === "phase" && job.phase !== event.phase) {
      if (job.kind === "run") store.set({ activeJob: { ...job, phase: /** @type {import("../types").RunPhase} */ (event.phase) } });
      else store.set({ activeJob: { ...job, phase: /** @type {import("../types").AcqPhase} */ (event.phase) } });
    }
  }

  /**
   * @param {JobKind} kind
   * @param {string} id
   */
  const find = (kind, id) => (current && current.kind === kind && current.id === id ? current : null);

  return {
    current: () => current,
    find,
    begin(kind, casePath) {
      const s = newStream(kind, null, casePath);
      current = s;
      notify(s);
      return {
        onEvent: (event) => {
          if (current === s) deliver(s, event);
        },
        bind(id) {
          s.id = id;
          s.version += 1;
          // Events that arrived before the id was known could not update the active job.
          const job = store.get().activeJob;
          if (job && activeJobId(job) === id && s.phase && job.phase !== s.phase) {
            if (job.kind === "run") store.set({ activeJob: { ...job, phase: /** @type {import("../types").RunPhase} */ (s.phase) } });
            else store.set({ activeJob: { ...job, phase: /** @type {import("../types").AcqPhase} */ (s.phase) } });
          }
          notify(s);
        },
        abandon() {
          if (current === s) current = null;
        },
      };
    },
    async attach(api, kind, id, casePath) {
      const existing = find(kind, id);
      if (existing && (existing.live || existing.finished)) return existing;
      const s = newStream(kind, id, casePath);
      s.partial = true;
      current = s;
      /** @type {(RunEvent | AcqEvent)[] | null} Events that arrive before the backlog. */
      let early = [];
      try {
        const { backlog } = await api.job_attach({ kind, id }, (event) => {
          if (current !== s) return;
          if (early) early.push(event);
          else deliver(s, event);
        });
        if (current !== s) return s;
        appendLog(s.log, backlog);
        const queued = early;
        early = null;
        s.version += 1;
        notify(s);
        for (const event of queued) deliver(s, event);
        return s;
      } catch (err) {
        if (current === s) current = null;
        throw err;
      }
    },
    subscribe(fn) {
      subscribers.add(fn);
      return () => {
        subscribers.delete(fn);
      };
    },
  };
}
