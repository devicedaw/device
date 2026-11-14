/*
 * dstudio_engine.h — C ABI between the Rust application core and the C++23
 * real-time audio engine.
 *
 * Contract (see docs/architecture.md):
 *  - The Rust core owns the authoritative project model. The engine only ever
 *    receives immutable *session snapshots* via se_session_publish().
 *  - se_session_publish() is called from a non-real-time thread. The engine
 *    copies the description, builds a render graph off the audio thread, and
 *    publishes it to the audio callback with an atomic pointer swap. Retired
 *    graphs are reclaimed on a non-RT thread (never freed in the callback).
 *  - Transport commands are pushed through a lock-free SPSC queue and are
 *    applied at the start of the next audio block while the device runs;
 *    with no device running they apply immediately (see additions below).
 *  - All timeline positions/lengths are in sample frames at the engine sample
 *    rate. Musical time (bars/beats/tempo) is converted by the Rust core.
 *  - Every function is safe to call from any non-RT thread, but calls on a
 *    single engine must not race with se_engine_destroy().
 *  - Strings are UTF-8, NUL terminated. Return value SE_OK (0) on success,
 *    negative se_result on failure; se_last_error() gives a message for the
 *    calling thread.
 */
#ifndef DSTUDIO_ENGINE_H
#define DSTUDIO_ENGINE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define SE_ABI_VERSION 1u

typedef enum se_result {
    SE_OK = 0,
    SE_ERR_INVALID_ARG = -1,
    SE_ERR_IO = -2,
    SE_ERR_UNSUPPORTED_FORMAT = -3,
    SE_ERR_DEVICE = -4,
    SE_ERR_NOT_FOUND = -5,
    SE_ERR_STATE = -6,
    SE_ERR_OUT_OF_MEMORY = -7
} se_result;

typedef struct se_engine se_engine;

typedef struct se_config {
    uint32_t abi_version;     /* must be SE_ABI_VERSION */
    double sample_rate;       /* requested; device may override (query se_engine_sample_rate) */
    uint32_t max_block_size;  /* largest block the engine preallocates for */
    uint32_t output_channels; /* 2 for the first milestone */
} se_config;

/* ---- lifecycle ---------------------------------------------------------- */
uint32_t se_abi_version(void);
se_engine* se_engine_create(const se_config* config); /* NULL on failure */
void se_engine_destroy(se_engine* engine);
const char* se_last_error(void); /* thread-local, valid until next call on this thread */

/* Open and start the platform default output device (Core Audio on macOS). */
int32_t se_device_start(se_engine* engine);
int32_t se_device_stop(se_engine* engine);
int32_t se_device_is_running(const se_engine* engine);
double se_engine_sample_rate(const se_engine* engine);

/* ---- media assets ------------------------------------------------------- */
typedef struct se_asset_info {
    uint64_t asset_id;
    uint64_t frames;          /* at engine sample rate (resampled on import) */
    uint32_t channels;        /* 1 or 2 */
    double source_sample_rate;
} se_asset_info;

/* Decode an audio file into engine memory, recognised by its content: WAV
 * (PCM 8/16/24/32, float 32/64), AIFF / AIFC, FLAC, Ogg Vorbis or MP3; mono
 * or stereo. Blocking; call from a worker thread. Resamples to the engine
 * rate. Anything else -> SE_ERR_UNSUPPORTED_FORMAT. */
int32_t se_asset_load_file(se_engine* engine, const char* utf8_path, se_asset_info* out_info);
int32_t se_asset_release(se_engine* engine, uint64_t asset_id);
int32_t se_asset_get_info(const se_engine* engine, uint64_t asset_id, se_asset_info* out_info);

/* A new asset holding asset_id's audio back to front (for reversed clips):
 * frame f of the copy is frame frames - 1 - f of the original. Same rate,
 * channels and source rate; release it like any asset. Copies the audio:
 * call from a non-audio thread. Unknown asset -> SE_ERR_NOT_FOUND. */
int32_t se_asset_reverse(se_engine* engine, uint64_t asset_id, se_asset_info* out_info);

/* Min/max peak pairs for waveform drawing, mixed to mono.
 * Writes up to `capacity` pairs (2 floats each) into out_minmax. */
int32_t se_asset_compute_peaks(const se_engine* engine, uint64_t asset_id,
                               uint64_t start_frame, uint32_t frames_per_peak,
                               float* out_minmax, size_t capacity, size_t* out_count);

/* The same for one channel (0 left, 1 right), for editors that draw a lane
 * per channel. A channel the asset lacks -> SE_ERR_INVALID_ARG. */
int32_t se_asset_compute_channel_peaks(const se_engine* engine, uint64_t asset_id, uint32_t channel,
                                       uint64_t start_frame, uint32_t frames_per_peak,
                                       float* out_minmax, size_t capacity, size_t* out_count);

/* ---- session snapshots -------------------------------------------------- */
typedef struct se_clip_desc {
    uint64_t clip_id;
    uint64_t asset_id;
    int64_t timeline_start; /* frame where the clip starts on the timeline */
    int64_t source_offset;  /* frame offset into the asset (trim start) */
    int64_t length;         /* frames played (trim end = offset + length) */
    float gain;             /* linear */
} se_clip_desc;

typedef struct se_track_desc {
    uint64_t track_id;
    float gain;  /* linear */
    float pan;   /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t _pad[6];
    const se_clip_desc* clips;
    uint32_t clip_count;
} se_track_desc;

typedef struct se_session_desc {
    const se_track_desc* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
} se_session_desc;

/* Copy the description and atomically publish it to the audio thread. */
int32_t se_session_publish(se_engine* engine, const se_session_desc* session);

/* ---- transport ---------------------------------------------------------- */
typedef struct se_transport_info {
    int64_t position;  /* frames */
    uint8_t playing;
    uint8_t _pad[7];
} se_transport_info;

int32_t se_transport_play(se_engine* engine);
int32_t se_transport_pause(se_engine* engine);
int32_t se_transport_stop(se_engine* engine); /* pause + return to position 0 */
int32_t se_transport_seek(se_engine* engine, int64_t frame);
int32_t se_transport_get(const se_engine* engine, se_transport_info* out_info);

/* ---- metering & stats --------------------------------------------------- */
typedef struct se_meter {
    uint64_t track_id; /* 0 = master */
    float peak_l, peak_r; /* linear, decaying peak hold since last read */
} se_meter;

/* Fills up to `capacity` meters (master first). */
int32_t se_meters_read(se_engine* engine, se_meter* out, size_t capacity, size_t* out_count);

typedef struct se_stats {
    uint64_t callbacks;
    uint64_t xruns_or_overloads;    /* callbacks exceeding their deadline */
    double last_callback_us;
    double max_callback_us;
    double avg_callback_us;
    double block_deadline_us;
} se_stats;
int32_t se_stats_get(const se_engine* engine, se_stats* out);
void se_stats_reset(se_engine* engine);

/* ---- offline rendering (tests, export) ---------------------------------- */
/* Render `frames` interleaved stereo frames using the same process path as
 * the device callback. Fails with SE_ERR_STATE while the device is running. */
int32_t se_render_offline(se_engine* engine, float* out_interleaved, uint32_t frames);

/* Render the whole session range to a 32-bit float WAV file. With plugin
 * delay compensation active (se_pdc_*), L extra frames are rendered and the
 * first L dropped, so timeline frame t is file frame t - start_frame. */
int32_t se_export_wav(se_engine* engine, const char* utf8_path, int64_t start_frame, int64_t end_frame);

/* ---- additions (ABI v1, append-only) ------------------------------------
 * Added after the first milestone without changing any existing struct or
 * signature; SE_ABI_VERSION stays 1. Callers that need them can probe with
 * a link-time check; there is no runtime feature query.
 *
 * Transport commands (play/pause/stop/seek and se_transport_set_loop /
 * se_metronome_set) are applied synchronously on the calling thread when no
 * device is running and no se_render_offline call is in progress, so
 * se_transport_get reflects them as soon as the call returns. While the
 * device runs they are queued and applied at the start of the next block. */

/* Non-destructive meter read. Per meter (master first, then tracks in
 * session order, same as se_meters_read) the max absolute peak over the last
 * ~50 ms of rendered audio (a ring of 5 x 10 ms buckets; a single block
 * longer than that counts as one bucket). Resets nothing and does not
 * interfere with se_meters_read. The window advances with rendered frames,
 * so it decays to 0 after ~50 ms of rendered silence (paused output counts),
 * and holds its value while nothing renders. Excludes the metronome. */
int32_t se_meters_peek(se_engine* engine, se_meter* out, size_t capacity, size_t* out_count);

/* Loop playback. When enabled and playing, a playhead in [start, end) (or
 * reaching start from before it) wraps sample-accurately from `end` back to
 * `start`, also mid-block. A playhead at or past `end` plays linearly.
 * Enabling requires start_frame < end_frame (else SE_ERR_INVALID_ARG);
 * when disabling, start/end are ignored. Disabling mid-play continues
 * linearly. se_export_wav ignores the loop. */
int32_t se_transport_set_loop(se_engine* engine, uint8_t enabled, int64_t start_frame, int64_t end_frame);

/* Metronome: a ~15 ms decaying sine click (1.2 kHz downbeat, 800 Hz other
 * beats) mixed into the master output after master gain, while playing.
 * Beat k sounds at timeline frame llround(k * frames_per_beat) (beat 0 at
 * frame 0; negative k in pre-roll), beats where k mod beats_per_bar == 0
 * are downbeats. Clicks ring out across block boundaries, loop wraps and
 * seeks; they stop on pause/stop/disable. With plugin delay compensation
 * the clicks are delayed by L like the music (heard with the beat they
 * belong to). Excluded from se_export_wav and from the meters. Validation: gain finite and >= 0, frames_per_beat
 * finite and >= 1, beats_per_bar >= 1 (only checked when enabled). */
typedef struct se_metronome_desc {
    uint8_t enabled;
    uint8_t _pad[3];
    float gain; /* linear */
    double frames_per_beat;
    uint32_t beats_per_bar;
    uint32_t _pad2;
} se_metronome_desc;

int32_t se_metronome_set(se_engine* engine, const se_metronome_desc* desc);

/* ---- MIDI notes + built-in instrument (ABI v1 addition) -----------------
 * se_session_publish_v2 is a superset of se_session_publish: every v2 track
 * carries the v1 fields (same meaning, same validation) plus `instrument`
 * and a note list. Tracks with instrument != 0 play their notes through the
 * engine's built-in polyphonic synthesizer (16 voices per track); the synth
 * output is summed with the track's audio clips and then goes through the
 * track's gain/pan/mute/solo and meters like any other track signal.
 * Notes on tracks with instrument == 0 are ignored. se_session_publish keeps
 * working and is equivalent to v2 with instrument = 0 and no notes.
 *
 * Note semantics: note-on at timeline frame `start`, note-off at `end`
 * (the release tail continues past it); both are sample-accurate. Notes may
 * be given in any order (the engine sorts by start). Validation: pitch
 * 0..127 (60 = C4, A4 = 69 = 440 Hz equal temperament), velocity 1..127,
 * NULL `notes` with note_count > 0 -> SE_ERR_INVALID_ARG. Notes with
 * end <= start are dropped.
 *
 * Republishing keeps sounding notes: voices are matched to notes by
 * (track_id, start, pitch); a matched voice keeps playing (its note-off
 * follows the new `end`), a voice whose note disappeared is released.
 * Seek, stop, pause and loop wraps release every sounding voice with a
 * ~5 ms fade; notes starting exactly at the new position are played, notes
 * already in progress there are not retriggered. se_export_wav renders
 * notes with its own private synth state. */
typedef struct se_note_desc {
    int64_t start;    /* note-on timeline frame */
    int64_t end;      /* note-off timeline frame (exclusive) */
    uint8_t pitch;    /* 0..127 */
    uint8_t velocity; /* 1..127 */
    uint8_t _pad[6];
} se_note_desc; /* 24 bytes, align 8 */

typedef struct se_track_desc_v2 {
    uint64_t track_id;
    float gain;  /* linear */
    float pan;   /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t instrument; /* != 0: render `notes` through the built-in synth */
    uint8_t _pad[5];
    const se_clip_desc* clips;
    uint32_t clip_count;
    uint32_t note_count;
    const se_note_desc* notes;
} se_track_desc_v2; /* 48 bytes, align 8 */

typedef struct se_session_desc_v2 {
    const se_track_desc_v2* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
} se_session_desc_v2; /* 16 bytes, align 8 */

int32_t se_session_publish_v2(se_engine* engine, const se_session_desc_v2* session);

/* ---- built-in synth parameters (ABI v1 addition) ------------------------
 * se_session_publish_v3 is a superset of se_session_publish_v2: every v3
 * track carries the v2 fields (same meaning, same validation) plus a pointer
 * to the parameters of its built-in synth. NULL (and every v1/v2 publish)
 * means the defaults, which se_synth_params_default() writes (they equal
 * dstudio_core::instrument::SynthParams::default()). Parameters are copied
 * into the render graph, so they change atomically with the rest of the
 * session; continuous parameters are smoothed by the engine (~10 ms), so a
 * knob sweep published as many snapshots does not zipper. Parameters on
 * non-instrument tracks are validated but unused.
 *
 * Validation (else SE_ERR_INVALID_ARG): enums in range, every float finite
 * and inside the documented range, octave -3..3, semitones -12..12,
 * polyphony 1..16, unison 1..4. Reserved/padding bytes are ignored.
 *
 * Enum values: waveform 0 saw, 1 square, 2 triangle, 3 sine;
 * filter_mode 0 low-pass, 1 high-pass, 2 band-pass. */
enum { SE_WAVE_SAW = 0, SE_WAVE_SQUARE = 1, SE_WAVE_TRIANGLE = 2, SE_WAVE_SINE = 3 };
enum { SE_FILTER_LOWPASS = 0, SE_FILTER_HIGHPASS = 1, SE_FILTER_BANDPASS = 2 };

typedef struct se_synth_params {
    uint8_t osc1_wave;     /* SE_WAVE_* */
    int8_t osc1_octave;    /* -3..3 */
    int8_t osc1_semitones; /* -12..12 */
    uint8_t osc2_wave;
    int8_t osc2_octave;
    int8_t osc2_semitones;
    uint8_t filter_mode; /* SE_FILTER_* */
    uint8_t lfo_wave;    /* SE_WAVE_* */
    uint8_t polyphony;   /* 1..16 (1 = mono, legato) */
    uint8_t unison;      /* 1..4 oscillator stacks per voice */
    uint8_t _pad[6];
    float osc1_shape;      /* 0..1: square pulse width 50%->5%, saw<->triangle morph, sine fold */
    float osc1_fine_cents; /* -100..100 */
    float osc1_level;      /* 0..1 */
    float osc2_shape;
    float osc2_fine_cents;
    float osc2_level;
    float cross_fm;    /* 0..1: osc2 -> osc1 linear FM */
    float sub_level;   /* 0..1: sine one octave below osc1 */
    float noise_level; /* 0..1: white noise */
    float filter_cutoff_hz;  /* 20..20000 (at C4 when key tracking is on) */
    float filter_resonance;  /* 0..1 */
    float filter_key_track;  /* 0..1 (1 = one octave per octave around C4) */
    float filter_env_amount; /* -1..1 (x 6 octaves of the filter envelope) */
    float amp_attack, amp_decay, amp_sustain, amp_release;         /* s 0.001..10, 0.001..10, 0..1, 0.001..20 */
    float filter_attack, filter_decay, filter_sustain, filter_release; /* same ranges */
    float lfo_rate_hz;        /* 0.01..20 */
    float lfo_to_pitch_cents; /* 0..1200 */
    float lfo_to_cutoff_oct;  /* 0..6 */
    float lfo_to_amp;         /* 0..1 tremolo depth */
    float glide;               /* portamento seconds 0..2 */
    float unison_detune_cents; /* total spread 0..50 */
    float output;              /* linear 0..2 */
} se_synth_params; /* 128 bytes, align 4 */

typedef struct se_track_desc_v3 {
    uint64_t track_id;
    float gain; /* linear */
    float pan;  /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t instrument; /* != 0: render `notes` through the built-in synth */
    uint8_t _pad[5];
    const se_clip_desc* clips;
    uint32_t clip_count;
    uint32_t note_count;
    const se_note_desc* notes;
    const se_synth_params* synth; /* NULL = defaults */
} se_track_desc_v3; /* 56 bytes, align 8 */

typedef struct se_session_desc_v3 {
    const se_track_desc_v3* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
} se_session_desc_v3; /* 16 bytes, align 8 */

int32_t se_session_publish_v3(se_engine* engine, const se_session_desc_v3* session);

/* Writes the default synth parameters (no-op for NULL). */
void se_synth_params_default(se_synth_params* out);


/* ---- audio input, monitoring and recording (ABI v1 addition) ------------
 * See docs/adr/0002-recording.md and engine/README.md ("Input, monitoring
 * and recording").
 *
 * se_session_publish_v4 is a superset of se_session_publish_v3: every v4
 * track carries the v3 fields (same meaning, same validation) plus its
 * record-arm / input-monitoring state and its device input routing.
 * v1/v2/v3 publishes are equivalent to v4 with arm = monitor = 0 and no
 * input.
 *
 * Input routing: input_mode SE_INPUT_MONO feeds device input channel
 * `input_channel` (0-based) to both sides of the track (then the track's
 * pan law), SE_INPUT_STEREO feeds channels input_channel / input_channel+1
 * to left / right (pan acts as balance, like stereo clips). Channels the
 * device does not have read as silence.
 *  - monitor != 0: the input is heard through the track (summed with its
 *    clips/notes, then gain/pan/mute/solo, meters, master), also while the
 *    transport is stopped.
 *  - arm != 0 and monitor == 0: the input is NOT heard, but the track meter
 *    shows the raw input level (pre-fader), so levels can be set before
 *    recording ("meter input when armed").
 *  - se_record_start captures every track with arm != 0 and an input.
 * Validation: input_mode <= SE_INPUT_STEREO, else SE_ERR_INVALID_ARG. */
enum { SE_INPUT_NONE = 0, SE_INPUT_MONO = 1, SE_INPUT_STEREO = 2 };

typedef struct se_track_desc_v4 {
    uint64_t track_id;
    float gain; /* linear */
    float pan;  /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t instrument;     /* != 0: render `notes` through the built-in synth */
    uint8_t arm;            /* armed for recording */
    uint8_t monitor;        /* input monitoring */
    uint8_t input_mode;     /* SE_INPUT_* */
    uint16_t input_channel; /* first device input channel, 0-based */
    const se_clip_desc* clips;
    uint32_t clip_count;
    uint32_t note_count;
    const se_note_desc* notes;
    const se_synth_params* synth; /* NULL = defaults */
} se_track_desc_v4; /* 56 bytes, align 8 */

typedef struct se_session_desc_v4 {
    const se_track_desc_v4* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
} se_session_desc_v4; /* 16 bytes, align 8 */

int32_t se_session_publish_v4(se_engine* engine, const se_session_desc_v4* session);

/* ---- devices ------------------------------------------------------------
 * Enumeration only reads device properties; it never opens a device (and
 * so never triggers the microphone-permission prompt). */
#define SE_DEVICE_STRING_MAX 256
#define SE_DEVICE_MAX_RATES 16

typedef struct se_device_info {
    char uid[SE_DEVICE_STRING_MAX];  /* stable id (Core Audio device UID), UTF-8 */
    char name[SE_DEVICE_STRING_MAX]; /* display name, UTF-8 (truncated) */
    uint32_t input_channels;
    uint32_t output_channels;
    uint8_t is_default_input;
    uint8_t is_default_output;
    uint8_t _pad[2];
    uint32_t sample_rate_count;
    double sample_rates[SE_DEVICE_MAX_RATES]; /* supported standard nominal rates, ascending */
} se_device_info; /* 656 bytes, align 8 */

/* Writes up to `capacity` devices; *out_count receives the total number of
 * devices (which may exceed capacity). */
int32_t se_device_list(se_device_info* out, size_t capacity, size_t* out_count);

/* Device setup (Settings > Audio). Takes effect at the next se_device_start;
 * if the device is running it is restarted with the new setup.
 *  - output_uid: NULL or "" = system default output.
 *  - input_enabled = 0: output only (no input device is opened, ever).
 *    input_enabled != 0: open input_uid (NULL or "" = system default
 *    input). Input is only ever opened by se_device_start / a running
 *    restart after this was requested; the default engine never opens one.
 *  - sample_rate: device nominal rate; 0 = leave it (output only) or the
 *    engine rate (with input). With an input it must equal the engine rate
 *    (the input path does not resample); output-only it may be any rate the
 *    output device supports (the output unit converts).
 *  - buffer_size: device IO buffer in frames, 0 = device default; else
 *    16..4096 and inside the output device's range.
 * Validation happens first, without opening anything: unknown uid ->
 * SE_ERR_NOT_FOUND, a device without the needed channels, an unsupported
 * rate/buffer or rate/input combination -> SE_ERR_INVALID_ARG.
 * If the output opens but the input does not (e.g. microphone permission
 * denied), output keeps running, the status reports no input and
 * input_error says why; a running restart then returns SE_ERR_DEVICE. */
typedef struct se_device_config {
    const char* output_uid;
    const char* input_uid;
    double sample_rate;
    uint32_t buffer_size;
    uint8_t input_enabled;
    uint8_t _pad[3];
} se_device_config; /* 32 bytes, align 8 */

int32_t se_device_configure(se_engine* engine, const se_device_config* config);

typedef struct se_device_status {
    char output_name[SE_DEVICE_STRING_MAX];
    char input_name[SE_DEVICE_STRING_MAX];  /* "" when no input is open */
    char input_error[SE_DEVICE_STRING_MAX]; /* why the configured input is not open, "" if none */
    uint8_t running;
    uint8_t input_open;
    uint8_t _pad[6];
    double sample_rate;             /* device nominal rate (engine rate when not running) */
    uint32_t buffer_size;           /* device IO buffer, frames */
    uint32_t input_channels;
    uint32_t output_channels;
    uint32_t input_latency_frames;  /* engine frames: device + safety offset + stream latency (input side) */
    uint32_t output_latency_frames; /* same, output side. Takes are shifted by input + output. */
    uint32_t _pad2;
} se_device_status; /* 808 bytes, align 8 */

int32_t se_device_status_get(se_engine* engine, se_device_status* out);

/* Silence all device input (monitoring, input meters and recordings get
 * zeros; recordings keep their timing). */
int32_t se_input_set_muted(se_engine* engine, uint8_t muted);

/* ---- recording ----------------------------------------------------------
 * se_record_start(dir): from the next audio block, captures the raw input
 * (pre-fader, pre-mute) of every track that is armed and has an input in
 * the published session, and starts the transport if it is stopped. The
 * set of tracks is fixed at start. The audio thread copies input into
 * preallocated per-track lock-free rings; a writer thread drains them into
 * 32-bit float WAV files in `dir` (created if missing) named
 * rec-<date>-<time>-<n>-t<track_id>-<take>.wav. A ring overrun (writer too
 * slow) drops that block for that track and counts it; the take ends there
 * and a new take starts with the next block that fits, so every take stays
 * sample-aligned.
 *
 * Takes: a take is a run of contiguous playback. A seek or stop ends the
 * current take; the next captured block starts a new take at its position.
 * Pause/play at the same position continues the take. A loop wrap does NOT
 * end the take: recording while looping writes linearly (no take lanes).
 * start_frame = timeline frame of the block's first sample minus the device
 * round-trip latency (input + output latency, se_device_status) minus the
 * plugin delay compensation L (se_pdc_report), so the take lines up with
 * what was heard; injected input has no device part (only L). A change of L
 * while recording ends the takes like a seek.
 *
 * se_record_stop: stops capturing (the transport keeps its state), waits
 * for the writer, finalizes every file and returns the number of takes,
 * readable with se_record_get_take until the next se_record_start.
 * Errors: start while recording / stop while not, no armed track with an
 * input, or a running device without an open input -> SE_ERR_STATE; dir not
 * creatable -> SE_ERR_IO; non-integer engine rate -> SE_ERR_UNSUPPORTED_FORMAT. */
int32_t se_record_start(se_engine* engine, const char* utf8_dir);
int32_t se_record_stop(se_engine* engine, size_t* out_take_count);

#define SE_PATH_MAX 1024
typedef struct se_recorded_take {
    uint64_t track_id;
    int64_t start_frame; /* timeline frame of the first sample (latency compensated) */
    uint64_t frames;
    uint32_t channels;   /* 1 (mono input) or 2 (stereo input) */
    uint32_t _pad;
    double sample_rate;  /* engine rate */
    char path[SE_PATH_MAX]; /* UTF-8 */
} se_recorded_take; /* 1064 bytes, align 8 */

int32_t se_record_get_take(se_engine* engine, size_t index, se_recorded_take* out);

/* Live view. Cheap, callable from any non-RT thread at UI rate. Every take
 * of the current recording, in start order (frames lag the audio thread by
 * at most one writer period, ~5 ms). *out_count = total takes. */
typedef struct se_live_take {
    uint64_t track_id;
    int64_t start_frame;
    uint64_t frames;
} se_live_take; /* 24 bytes, align 8 */

int32_t se_record_state(se_engine* engine, uint8_t* out_active, se_live_take* out, size_t capacity, size_t* out_count);

/* Min/max pairs (mono mix) of the newest take of `track_id` while
 * recording, from its first frame; resolution 64 frames (frames_per_peak
 * is rounded to whole 64-frame buckets). Count 0 when there is none. */
int32_t se_record_peaks(se_engine* engine, uint64_t track_id, uint32_t frames_per_peak, float* out_minmax,
                        size_t capacity, size_t* out_count);

typedef struct se_record_stats {
    uint64_t overruns;       /* blocks dropped because a ring was full (all tracks) */
    uint64_t dropped_frames; /* frames lost to overruns (sum over tracks) */
    uint64_t write_errors;   /* takes that could not be written completely */
    uint64_t takes;          /* takes started so far */
} se_record_stats; /* 32 bytes */

/* Counters of the current recording, or of the last one after stop
 * (reset by se_record_start). */
int32_t se_record_stats_get(se_engine* engine, se_record_stats* out);

/* se_render_offline with injected device input: `input_interleaved` holds
 * `frames` frames of `input_channels` (1..32) interleaved channels, used
 * exactly like device input (routing, monitoring, meters, recording with 0
 * latency compensation). Same rules as se_render_offline. */
int32_t se_render_offline_with_input(se_engine* engine, const float* input_interleaved, uint32_t input_channels,
                                     float* out_interleaved, uint32_t frames);


/* ---- plugins: device chains (ABI v1 addition) ---------------------------
 * See docs/adr/0003-plugins.md and engine/README.md ("Plugins").
 *
 * se_session_publish_v5 is a superset of se_session_publish_v4: every v5
 * track carries the v4 fields (same meaning, same validation) plus its insert
 * chain (`devices`, in processing order) and, on instrument tracks, an
 * instrument plugin that replaces the built-in synth. v1..v4 publishes are
 * equivalent to v5 without devices (they retire every plugin instance).
 *
 * THREADING: se_session_publish_v5 must be called on the app's main thread:
 * a device id seen for the first time is instantiated synchronously during
 * the call (format rules: VST3/CLAP main-thread APIs; AU via the synchronous
 * AudioComponentInstanceNew), prepared at the engine rate / max block size
 * with stereo in/out, and given its saved `state`. Instances are kept across
 * republishes keyed by device_id; `state` is only read when an instance is
 * created (it may be NULL for a live device, see se_plugin_is_live). A device
 * id that disappears is retired: its editor closes, and the instance is
 * destroyed on the main thread (inside a later publish or se_plugin_* call)
 * once the audio thread can no longer reach it.
 * A device that cannot be instantiated (missing bundle, unknown id, load or
 * setup failure, unsupported bus layout) passes audio through unchanged
 * (an instrument plugin renders silence); the reason is reported by
 * se_plugin_load_errors. Instantiation is not retried while the device id
 * stays published with the same plugin.
 *
 * Processing per track: clips + (instrument plugin | built-in synth) +
 * monitored input -> insert chain (enabled devices only) -> gain/pan/
 * mute/solo -> meters -> master. Insert chains keep running while the
 * transport is stopped (tails ring out), also on muted tracks (not heard).
 * Instrument plugins receive the track's notes as sample-accurate note
 * on/off events; seek, stop, pause and loop wraps send note-offs for every
 * sounding note (plus MIDI CC 123 to Audio Units).
 *
 * Validation (else SE_ERR_INVALID_ARG): device_id != 0 and unique within the
 * session (inserts and instrument plugins together), format <= SE_PLUGIN_CLAP,
 * plugin_id non-NULL, state non-NULL when state_len > 0. */
enum { SE_PLUGIN_AU = 0, SE_PLUGIN_VST3 = 1, SE_PLUGIN_CLAP = 2 };

typedef struct se_device_desc {
    uint64_t device_id;     /* dstudio_core DeviceId, != 0 */
    const char* name;       /* display name (editor window title); may be NULL */
    const char* plugin_id;  /* AU "type subtype manufacturer" (e.g. "aufx lpas appl"),
                               VST3 class UID (32 hex chars), CLAP plugin id */
    const char* path;       /* bundle path (VST3/CLAP, required there); ignored for AU */
    const uint8_t* state;   /* saved state (format-specific, see README), NULL = none */
    size_t state_len;
    uint8_t format;         /* SE_PLUGIN_* */
    uint8_t enabled;        /* 0 = bypassed (skipped, audio passes unchanged) */
    uint8_t _pad[6];
} se_device_desc; /* 56 bytes, align 8 */

typedef struct se_track_desc_v5 {
    uint64_t track_id;
    float gain; /* linear */
    float pan;  /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t instrument;
    uint8_t arm;
    uint8_t monitor;
    uint8_t input_mode;     /* SE_INPUT_* */
    uint16_t input_channel;
    const se_clip_desc* clips;
    uint32_t clip_count;
    uint32_t note_count;
    const se_note_desc* notes;
    const se_synth_params* synth;           /* NULL = defaults */
    const se_device_desc* devices;          /* insert chain, processing order */
    uint32_t device_count;
    uint32_t _pad2;
    const se_device_desc* instrument_plugin; /* NULL = built-in synth; used only when instrument != 0 */
    const char* track_name;                  /* editor window titles; may be NULL */
} se_track_desc_v5; /* 88 bytes, align 8 */

typedef struct se_session_desc_v5 {
    const se_track_desc_v5* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
} se_session_desc_v5; /* 16 bytes, align 8 */

int32_t se_session_publish_v5(se_engine* engine, const se_session_desc_v5* session);

/* 1 if `device_id` has a live (or failed, not to be retried) instance for
 * the currently published plugin, else 0. Lets the caller skip decoding
 * saved state for devices that will not be instantiated. Any thread. */
int32_t se_plugin_is_live(se_engine* engine, uint64_t device_id);

/* Load errors of currently published devices. Writes up to `capacity`;
 * *out_count = total. Any non-RT thread. */
#define SE_PLUGIN_MESSAGE_MAX 256
typedef struct se_plugin_message {
    uint64_t device_id;
    char message[SE_PLUGIN_MESSAGE_MAX]; /* UTF-8, truncated */
} se_plugin_message; /* 264 bytes, align 8 */
int32_t se_plugin_load_errors(se_engine* engine, se_plugin_message* out, size_t capacity, size_t* out_count);

/* Plugin state, two steps. MAIN THREAD (VST3/CLAP save state on the main
 * thread; safe while audio runs). se_plugin_save_states saves the state of
 * every live instance into an engine-owned buffer and returns how many;
 * se_plugin_saved_state reads entry `index`. Pointers stay valid until the
 * next se_plugin_save_states or se_engine_destroy. Instances whose save
 * fails are left out. Formats (opaque to callers, documented in README):
 * AU = binary plist of kAudioUnitProperty_ClassInfo; VST3 = "SVS3"
 * container of component + controller state; CLAP = the clap.state stream. */
int32_t se_plugin_save_states(se_engine* engine, size_t* out_count);
int32_t se_plugin_saved_state(se_engine* engine, size_t index, uint64_t* out_device_id, const uint8_t** out_data,
                              size_t* out_len);

/* Saves the state of ONE live instance (same formats as se_plugin_save_states)
 * into the engine-owned buffer, replacing its contents: afterwards
 * se_plugin_saved_state(index 0) reads it. MAIN THREAD. SE_ERR_NOT_FOUND: no
 * live instance for the device; SE_ERR_STATE: the plugin failed to save (the
 * reason is the last error). A clip render calls it once per UI tick so the
 * main thread is never held for the whole chain. */
int32_t se_plugin_save_state(se_engine* engine, uint64_t device_id);

/* ---- plugins in offline renders on a worker thread (ABI v1 addition)
 * See engine/README.md ("Plugins in offline renders").
 *
 * A private engine without a device renders a project on a worker thread, but
 * plugins want their creation, state, latency and teardown on the main thread
 * (CLAP calls the creating thread its main thread, and publish creates). So
 * the main thread prepares the instances ahead of the publish and the worker
 * only publishes and renders:
 *   1. MAIN THREAD (like se_session_publish_v5, which creates instances the
 *      same way), per device, one device per UI tick: se_plugin_prepare
 *      (instantiates now with `state`, the live instance's current state,
 *      stereo in/out at the engine rate and max block size, the saved state
 *      applied, activated). SE_ERR_NOT_FOUND = the plugin failed to load; the
 *      reason is the last error. `instrument` is 1 for a track's instrument
 *      plugin (as in se_track_desc_v5.instrument_plugin), 0 for an insert.
 *   2. se_plugin_set_detached(engine, 1) before the worker publishes.
 *   3. WORKER: se_session_publish_* finds the prepared instances by device_id
 *      (same plugin, same role) and reuses them; se_render_offline processes
 *      them (process() may run on any thread). Detached, the engine never
 *      instantiates (a device id that was not prepared fails to load: pass-
 *      through / silent instrument, reported by se_plugin_load_errors), never
 *      restarts, polls, calls back or destroys instances, and se_plugin_idle
 *      only attaches finished warp renders.
 *   4. MAIN THREAD: se_engine_destroy tears the instances down once the
 *      worker is done with the engine. */
int32_t se_plugin_prepare(se_engine* engine, const se_device_desc* device, uint8_t instrument);
int32_t se_plugin_set_detached(se_engine* engine, uint8_t detached);

/* Native editor windows (macOS: NSWindow titled "<plugin>: <track>").
 * MAIN THREAD ONLY (else SE_ERR_STATE). Opening an open editor brings its
 * window to the front. Unknown/failed device -> SE_ERR_NOT_FOUND; a plugin
 * without an editor -> SE_ERR_UNSUPPORTED_FORMAT (AUs fall back to the
 * generic AUGenericView). Closing the window with its close button closes
 * the editor (se_plugin_editor_is_open then returns 0). Editors of retired
 * instances close automatically. */
int32_t se_plugin_editor_open(se_engine* engine, uint64_t device_id);
int32_t se_plugin_editor_close(se_engine* engine, uint64_t device_id);
int32_t se_plugin_editor_is_open(se_engine* engine, uint64_t device_id); /* 1 / 0 */

/* Processing latency reported by the plugin, in frames (0 if unknown or not
 * live), queried live from the plugin. Delay compensation uses the value
 * cached at instantiation / the last se_plugin_idle (see se_pdc_*). Main
 * thread. */
int32_t se_plugin_latency(se_engine* engine, uint64_t device_id, uint32_t* out_frames);

/* Main-thread housekeeping: destroys retired instances the audio thread can
 * no longer reach and runs pending CLAP on_main_thread callbacks. Also done
 * inside se_session_publish_v5 and the other se_plugin_* calls; on macOS
 * CLAP callbacks are additionally dispatched to the main queue.
 * Delay compensation: also services plugin restart requests (CLAP
 * request_restart, VST3 restartComponent(kLatencyChanged)) by
 * deactivating/re-activating the plugin here (it is bypassed meanwhile),
 * polls every live instance's latency and, when a latency changed,
 * re-plans delay compensation. Call it regularly (e.g. 10-60 Hz) from the
 * main thread so latency changes take effect. */
int32_t se_plugin_idle(se_engine* engine);

/* ---- plugin delay compensation (ABI v1 addition) -------------------------
 * See docs/adr/0004-delay-compensation.md and engine/README.md ("Plugin
 * delay compensation").
 *
 * Per track, lat_i = sum of the reported latencies of its insert devices
 * (bypassed ones included: a bypassed slot delays by its latency instead,
 * so toggling bypass never shifts timing) + its instrument plugin's latency.
 * L = max lat_i over all tracks (muted ones included). Each track is delayed
 * by L - lat_i, so all tracks line up, and the whole mix (and the metronome)
 * is late by L relative to the transport position. Tracks monitoring input
 * delay only their playback source (input monitoring stays low latency).
 * Recorded takes are placed L earlier (on top of the device latency);
 * se_export_wav renders L extra frames and drops the first L, so exports
 * are timeline-aligned. Seek and stop flush the delay lines; pause lets
 * them ring out; loop wraps keep them running.
 *
 * Each delay is clamped to SE_PDC_MAX_DELAY_FRAMES (reported as `clamped`).
 * Enabled by default. Disabled, the engine renders exactly as without PDC
 * (bit-identical). Any non-RT thread; uses the latencies cached on the main
 * thread (publish / se_plugin_idle). */
#define SE_PDC_MAX_DELAY_FRAMES 131072u /* 2^17, ~2.7 s at 48 kHz */

int32_t se_pdc_set_enabled(se_engine* engine, uint8_t enabled);
int32_t se_pdc_is_enabled(const se_engine* engine); /* 1 / 0 */

typedef struct se_pdc_report {
    uint32_t total_frames;    /* applied L (0 when disabled) */
    uint32_t uncapped_frames; /* L before clamping (computed even when disabled) */
    uint32_t track_count;     /* tracks in the published session */
    uint8_t enabled;
    uint8_t clamped;          /* some delay exceeded SE_PDC_MAX_DELAY_FRAMES (only when enabled) */
    uint8_t _pad[2];
} se_pdc_report; /* 16 bytes, align 4 */

typedef struct se_pdc_track {
    uint64_t track_id;
    uint32_t latency_frames; /* lat_i (reported even when disabled) */
    uint32_t delay_frames;   /* compensation applied to the track: L - lat_i (0 when disabled) */
} se_pdc_track; /* 16 bytes, align 8 */

/* Report for the most recently published session (or re-plan). Writes up
 * to `capacity` tracks in session order; *out_count = track count. */
int32_t se_pdc_report_get(se_engine* engine, se_pdc_report* out, se_pdc_track* tracks, size_t capacity,
                          size_t* out_count);

/* What is being heard: the transport position minus the applied L and minus
 * the output device latency (se_device_status.output_latency_frames; 0
 * offline / while the device is stopped), while playing. While paused or
 * stopped this is the transport position. Block granularity (like
 * se_transport_get); L is that of the most recently published plan. */
int32_t se_transport_audible_position(const se_engine* engine, int64_t* out_frame);

/* ---- parameter automation (ABI v1 addition) -----------------------------
 * See docs/adr/0005-automation.md and engine/README.md ("Automation").
 *
 * se_session_publish_v6 is a superset of se_session_publish_v5: every v6
 * track carries the v5 fields (same meaning, same validation, same main-
 * thread rule) plus its automation lanes; the session adds master lanes.
 * v1..v5 publishes are equivalent to v6 without lanes.
 *
 * A lane is a list of points (timeline frame, plain value, curve) sorted by
 * frame. Evaluation (the contract of dstudio_core::automation, bit-for-bit):
 *   t <= first.frame        -> first.value
 *   t >= last.frame         -> last.value (several points at one frame: the
 *                              later one wins, except at the very first frame)
 *   a.frame <= t < b.frame  -> lerp(a.value, b.value, shape(s, a.curve)),
 *       s = (float)(t - a.frame) / (float)(b.frame - a.frame),
 *       shape(s, c) = c == 0 ? s : c > 0 ? s^(1 + 3c) : 1 - (1 - s)^(1 - 3c),
 *       lerp geometric (exp of the lerp of logs) when `logarithmic` != 0 and
 *       both values are > 0, else linear; all in float32.
 * An enabled lane with points REPLACES the static value of its target:
 *   SE_AUTO_VOLUME: dB, gain = 10^(dB/20), <= -60 dB is silence; replaces the
 *                   track's (or master's) linear `gain`. Per sample: exact at
 *                   every point and at least every 32 samples, linear gain
 *                   ramps in between.
 *   SE_AUTO_PAN:    -1..1 (clamped), replaces `pan`; same rate as volume.
 *   SE_AUTO_SYNTH:  `synth_param` (SE_SYNTH_P_*), value in the se_synth_params
 *                   unit and range; replaces that field of the track's
 *                   built-in synth, evaluated every 16 timeline frames and
 *                   applied through the synth's ~10 ms parameter smoothing.
 *   SE_AUTO_PLUGIN: `device_id` (an insert or the instrument plugin of the
 *                   same track) + `param_id` (VST3 ParamID, CLAP clap_id, AU
 *                   AudioUnitParameterID), value normalized 0..1 (clamped).
 *                   Sent as sample-offset parameter events (VST3
 *                   IParameterChanges, CLAP CLAP_EVENT_PARAM_VALUE with
 *                   plain = min + n * (max - min), AU AudioUnitScheduleParameters)
 *                   at the start of every processed block, every 64 timeline
 *                   frames and at every point (at most 64 points per lane and
 *                   block). A lane whose device or parameter does not exist is
 *                   inactive.
 * Lanes run on the timeline frame the audio they act on was read at: with
 * delay compensation active, a stage that processes audio d frames late (the
 * track fader: L; an insert: the latency of everything before it) evaluates
 * its lanes d frames earlier, following loop wraps, so automation stays
 * aligned with the audio. While stopped, lanes hold the value at the
 * playhead. se_export_wav renders automation.
 * Several lanes with the same target on one track: the last one wins. Master
 * lanes: only SE_AUTO_VOLUME is used (others are ignored).
 * Validation (else SE_ERR_INVALID_ARG): target <= SE_AUTO_MONO (see below
 * for SE_AUTO_BUILTIN and the switch and channel-strip targets), flags
 * only SE_AUTO_FLAG_*, synth_param < SE_SYNTH_P_COUNT, points non-NULL when point_count > 0,
 * frames non-decreasing, values and curves finite, curve in -1..1, synth
 * values inside the parameter's se_synth_params range. Lanes without points
 * are ignored. */
enum { SE_AUTO_VOLUME = 0, SE_AUTO_PAN = 1, SE_AUTO_SYNTH = 2, SE_AUTO_PLUGIN = 3 };

/* Built-in synth parameters, in dstudio_core::automation::SynthParam::ALL order. */
enum {
    SE_SYNTH_P_OSC1_LEVEL = 0, SE_SYNTH_P_OSC1_SHAPE, SE_SYNTH_P_OSC1_FINE, SE_SYNTH_P_OSC2_LEVEL,
    SE_SYNTH_P_OSC2_SHAPE, SE_SYNTH_P_OSC2_FINE, SE_SYNTH_P_CROSS_FM, SE_SYNTH_P_SUB_LEVEL, SE_SYNTH_P_NOISE_LEVEL,
    SE_SYNTH_P_FILTER_CUTOFF, SE_SYNTH_P_FILTER_RESONANCE, SE_SYNTH_P_FILTER_KEY_TRACK, SE_SYNTH_P_FILTER_ENV_AMOUNT,
    SE_SYNTH_P_LFO_RATE, SE_SYNTH_P_LFO_TO_PITCH, SE_SYNTH_P_LFO_TO_CUTOFF, SE_SYNTH_P_LFO_TO_AMP, SE_SYNTH_P_GLIDE,
    SE_SYNTH_P_UNISON_DETUNE, SE_SYNTH_P_OUTPUT,
    SE_SYNTH_P_COUNT /* 20 */
};

typedef struct se_auto_point {
    int64_t frame; /* timeline frame */
    float value;   /* plain value in the target's unit */
    float curve;   /* shape of the segment to the next point, -1..1 (0 = linear) */
} se_auto_point;   /* 16 bytes, align 8 */

typedef struct se_auto_lane_desc {
    uint8_t target;       /* SE_AUTO_* */
    uint8_t logarithmic;  /* != 0: geometric interpolation (frequency-like targets) */
    uint16_t synth_param; /* SE_AUTO_SYNTH: SE_SYNTH_P_* */
    uint32_t param_id;    /* SE_AUTO_PLUGIN: format parameter id */
    uint64_t device_id;   /* SE_AUTO_PLUGIN / BUILTIN / DEVICE_ON: the device (same track) */
    const se_auto_point* points;
    uint32_t point_count;
    uint32_t flags;       /* SE_AUTO_FLAG_* (was padding: 0 = none) */
} se_auto_lane_desc; /* 32 bytes, align 8 */

typedef struct se_track_desc_v6 {
    uint64_t track_id;
    float gain; /* linear */
    float pan;  /* -1 .. +1, constant-power */
    uint8_t mute;
    uint8_t solo;
    uint8_t instrument;
    uint8_t arm;
    uint8_t monitor;
    uint8_t input_mode;     /* SE_INPUT_* */
    uint16_t input_channel;
    const se_clip_desc* clips;
    uint32_t clip_count;
    uint32_t note_count;
    const se_note_desc* notes;
    const se_synth_params* synth;           /* NULL = defaults */
    const se_device_desc* devices;          /* insert chain, processing order */
    uint32_t device_count;
    uint32_t _pad2;
    const se_device_desc* instrument_plugin; /* NULL = built-in synth; used only when instrument != 0 */
    const char* track_name;                  /* editor window titles; may be NULL */
    const se_auto_lane_desc* lanes;          /* automation lanes (enabled ones only) */
    uint32_t lane_count;
    uint32_t _pad3;
} se_track_desc_v6; /* 104 bytes, align 8 */

typedef struct se_session_desc_v6 {
    const se_track_desc_v6* tracks;
    uint32_t track_count;
    float master_gain; /* linear */
    const se_auto_lane_desc* master_lanes; /* master volume automation */
    uint32_t master_lane_count;
    uint32_t _pad;
} se_session_desc_v6; /* 32 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v6(se_engine* engine, const se_session_desc_v6* session);

/* TESTING: evaluates `lane` (validated like a published lane) at `count`
 * timeline frames with the engine's evaluation code. Any thread. */
int32_t se_automation_eval(const se_auto_lane_desc* lane, const int64_t* frames, size_t count, float* out_values);

/* ---- audio clip warping (ABI v1 addition) --------------------------------
 * See engine/README.md ("Clip warping").
 *
 * se_session_publish_v7 is a superset of se_session_publish_v6: the session
 * carries v6 unchanged plus a list of warped clips. A clip listed there
 * (by clip_id, which must name a clip of the session, once) is no longer
 * played 1:1 from its asset: its timeline_start and length still give the
 * timeline frames it covers (length is NOT clamped to the asset, and
 * source_offset is ignored), and the warp map gives the source position of
 * every timeline frame. Clips not listed play as in v6.
 *
 * Warp map: points (timeline frame, source frame at the engine rate,
 * fractional) with strictly increasing frames and sources, at least two.
 * source(t) is linear between points and extends the first / last segment
 * beyond them.
 *
 * Modes:
 *   SE_WARP_REPITCH          varispeed: reads source(t) with band-limited
 *                            interpolation, so pitch follows the local rate.
 *                            transpose is ignored.
 *   SE_WARP_SLICE            transient slicing for drums: each slice (from
 *                            slices[k].source to the next slice or the asset
 *                            end) starts at the timeline frame where the map
 *                            reaches its source and plays at the transpose
 *                            rate (2^(transpose/12)), with a 2 ms fade where it
 *                            runs out and a 2 ms crossfade where the next slice
 *                            cuts it.
 *   SE_WARP_STRETCH          phase-vocoder time stretch (signalsmith-stretch)
 *                            following the map, pitch kept, then transposed.
 *   SE_WARP_STRETCH_FORMANT  SE_WARP_STRETCH with formant compensation, so a
 *                            transposed voice keeps its character.
 *   SE_WARP_AUTO             per passage: slices flagged percussive play as
 *                            SE_WARP_SLICE, the others as SE_WARP_STRETCH, with
 *                            a 3 ms crossfade where the choice changes.
 * The stretch modes run on per-track voices the engine allocates at publish
 * time and keeps across republishes; rendering never allocates or locks.
 * A voice restarts (aligned, without pre-roll latency) at a clip start,
 * whenever playback jumps, and when a republished map moves the source under
 * it. Meanwhile a background thread renders each stretch-mode clip ahead of
 * time (renders are reused for any clip with the same asset, mode, transpose,
 * map and slices relative to its start, so moving a clip or changing its gain
 * keeps them); se_plugin_idle swaps a finished render in, after which the clip
 * plays it at the cost of a copy (crossfading from the voice over one block
 * when it is playing). se_export_wav uses the renders that are attached and
 * private voices for the rest. Validation (else SE_ERR_INVALID_ARG): mode <= SE_WARP_AUTO,
 * transpose finite in -48..48, point and slice arrays non-NULL when counted,
 * point_count >= 2, frames strictly increasing, sources finite and strictly
 * increasing, slice sources finite and strictly increasing, clip ids known
 * and unique. */
enum {
    SE_WARP_REPITCH = 0,
    SE_WARP_SLICE = 1,
    SE_WARP_STRETCH = 2,
    SE_WARP_STRETCH_FORMANT = 3,
    SE_WARP_AUTO = 4
};

typedef struct se_warp_point {
    int64_t frame;  /* timeline frame */
    double source;  /* source frame (engine rate, fractional) */
} se_warp_point;    /* 16 bytes, align 8 */

typedef struct se_warp_slice {
    double source;      /* source frame where the slice starts (a transient) */
    uint8_t percussive; /* SE_WARP_AUTO: play this slice by slicing */
    uint8_t _pad[7];
} se_warp_slice;        /* 16 bytes, align 8 */

typedef struct se_clip_warp_desc {
    uint64_t clip_id;
    uint8_t mode;     /* SE_WARP_* */
    uint8_t _pad[3];
    float transpose;  /* semitones */
    const se_warp_point* points;
    uint32_t point_count;
    uint32_t slice_count;
    const se_warp_slice* slices; /* SE_WARP_SLICE / SE_WARP_AUTO, sorted by source */
} se_clip_warp_desc; /* 40 bytes, align 8 */

typedef struct se_session_desc_v7 {
    se_session_desc_v6 v6;
    const se_clip_warp_desc* warps;
    uint32_t warp_count;
    uint32_t _pad;
} se_session_desc_v7; /* 48 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v7(se_engine* engine, const se_session_desc_v7* session);

/* ---- audio clip looping (ABI v1 addition) -------------------------------
 * See engine/README.md ("Clip looping").
 *
 * se_session_publish_v8 is se_session_publish_v7 plus clip loops. A clip
 * listed there (by clip_id, which must name a clip of the session, once)
 * loops. Content frame c is the clip-relative frame the clip would play
 * without the loop: 1:1 clips read source_offset + c, warped clips read the
 * map at timeline_start + c. At clip-relative frame r the clip plays content
 * frame r while r < loop_end, then loop_start + (r - loop_end) mod
 * (loop_end - loop_start), until timeline_start + length. So the first pass
 * runs from the clip start to the loop end, and the loop repeats after it.
 * A looped 1:1 clip's length is not clamped to the asset; loop_end is (and
 * to length). A loop left empty by the clamps is dropped. A warped clip's
 * render covers frames below loop_end only. Wraps are hard cuts, and a
 * stretch voice restarts at each wrap. Rendering never allocates or locks.
 * Validation (else SE_ERR_INVALID_ARG): loops non-NULL when counted,
 * 0 <= loop_start < loop_end, clip ids known and unique. */
typedef struct se_clip_loop_desc {
    uint64_t clip_id;
    int64_t loop_start; /* content frames from the clip start */
    int64_t loop_end;
} se_clip_loop_desc; /* 24 bytes, align 8 */

typedef struct se_session_desc_v8 {
    se_session_desc_v7 v7;
    const se_clip_loop_desc* loops;
    uint32_t loop_count;
    uint32_t _pad;
} se_session_desc_v8; /* 64 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v8(se_engine* engine, const se_session_desc_v8* session);

/* Warp renders queued or running (0 = every stretch-mode clip of the latest
 * publish has its render; call se_plugin_idle to attach finished ones). Any
 * thread. */
int32_t se_warp_renders_pending(const se_engine* engine, size_t* out_count);

/* Transient (onset) detection on a loaded asset, for warp markers and
 * slicing. Spectral-flux onsets with adaptive peak picking, each moved to
 * the start of its attack. strength: 0..1 relative to the strongest onset.
 * percussive: the audio from this onset to the next decays like a hit (its
 * tail sits at least 12 dB under its attack; smoothed over neighbours).
 * Writes up to `capacity` onsets sorted by frame (engine-rate frames);
 * *out_count = total found. CPU-bound: call from a worker thread, never
 * from the audio thread. Unknown asset -> SE_ERR_NOT_FOUND. */
typedef struct se_onset {
    int64_t frame;
    float strength;
    uint8_t percussive;
    uint8_t _pad[3];
} se_onset; /* 16 bytes, align 8 */

int32_t se_asset_detect_onsets(se_engine* engine, uint64_t asset_id, se_onset* out, size_t capacity,
                               size_t* out_count);

/* ---- plugin parameters ---------------------------------------------------
 * MAIN THREAD, i.e. the thread that publishes sessions (VST3/CLAP parameter
 * queries are main-thread APIs; like se_session_publish_v5 this is not
 * enforced). Unknown device or a device that failed to load ->
 * SE_ERR_NOT_FOUND; unknown parameter -> SE_ERR_NOT_FOUND. */
#define SE_PARAM_STRING_MAX 128
typedef struct se_plugin_param_info {
    uint32_t id;                /* VST3 ParamID, CLAP clap_id, AU AudioUnitParameterID */
    uint32_t steps;             /* discrete steps (0 = continuous) */
    float default_normalized;   /* 0..1 */
    uint8_t automatable;
    uint8_t _pad[3];
    char name[SE_PARAM_STRING_MAX];  /* UTF-8, truncated */
    char group[SE_PARAM_STRING_MAX]; /* VST3 unit, CLAP module, AU clump; may be "" */
} se_plugin_param_info; /* 272 bytes, align 4 */

/* Parameters of a live instance (hidden ones are left out), in the plugin's
 * order. Writes up to `capacity`; *out_count = total. */
int32_t se_plugin_params(se_engine* engine, uint64_t device_id, se_plugin_param_info* out, size_t capacity,
                         size_t* out_count);
/* Current normalized value (VST3: the controller's value; CLAP: get_value;
 * AU: AudioUnitGetParameter). */
int32_t se_plugin_param_value(se_engine* engine, uint64_t device_id, uint32_t param_id, float* out_normalized);
/* Display text of a normalized value ("-6.0 dB"): VST3 getParamStringByValue,
 * CLAP value_to_text, AU kAudioUnitProperty_ParameterStringFromValue or the
 * value with its unit. Writes a NUL-terminated, truncated string. */
int32_t se_plugin_param_text(se_engine* engine, uint64_t device_id, uint32_t param_id, float normalized, char* out,
                             size_t capacity);

/* Parameter changes made in plugin editors (for "write" automation). Every
 * format reports into one lock-free queue (any thread, also the audio
 * thread): VST3 IComponentHandler beginEdit/performEdit/endEdit, CLAP
 * output events of process() and params.flush() (CLAP_EVENT_PARAM_GESTURE_
 * BEGIN/END and CLAP_EVENT_PARAM_VALUE; se_plugin_idle flushes when nothing
 * processes), AU parameter-change and begin/end-gesture events. A gesture
 * begin and end is its own record (flag set, value = the parameter's value
 * at that moment); value changes in between have no flag. `frame` = the
 * transport position when the change was observed (sample-accurate for
 * CLAP events from process()). Changes made by automation or by
 * se_session_publish are not reported. The queue holds 8192 records; on
 * overflow the newest are dropped. se_plugin_take_param_changes drains up to
 * `capacity` records in order (MAIN THREAD, like the other se_plugin_*). */
typedef struct se_plugin_param_change {
    uint64_t device_id;
    int64_t frame;
    uint32_t param_id;
    float normalized;
    uint8_t gesture_begin;
    uint8_t gesture_end;
    uint8_t _pad[6];
} se_plugin_param_change; /* 32 bytes, align 8 */

int32_t se_plugin_take_param_changes(se_engine* engine, se_plugin_param_change* out, size_t capacity,
                                     size_t* out_count);

/* ---- built-in devices (ABI v1 addition) ----------------------------------
 * See docs/devices.md and engine/src/devices/.
 *
 * A built-in device is an insert in a v5/v6 device chain with
 * format = SE_PLUGIN_BUILTIN and plugin_id = its registry id (for example
 * "compressor", see se_builtin_registry). It goes through the same instance
 * lifecycle as a plugin: kept across republishes by device_id, bypass via
 * `enabled`, delay compensation from its reported latency; a registry id
 * the engine does not know passes audio through and is reported by
 * se_plugin_load_errors. It has no editor window.
 *
 * `state` / `state_len` carry an se_builtin_state header followed by
 * value_count float32 plain parameter values in the registry's parameter
 * order. Unlike plugin state it is read on EVERY publish: the values become
 * the device's current parameters (lock-free hand-over, applied at the next
 * block, smoothed by the processor). Missing trailing values keep the
 * processor's current values; state == NULL keeps every current value.
 * Validation (else SE_ERR_INVALID_ARG): state_len >= 16, magic ==
 * SE_BUILTIN_STATE_MAGIC, state_len == 16 + 4 * value_count, values finite.
 *
 * Sidechain: sidechain_track_id names another track of the session whose
 * post-fader output feeds the device's detector. Tracks are processed so
 * that sources come before the devices that read them; a sidechain that
 * would form a cycle, names the device's own track or an unknown track, or
 * exceeds SE_BUILTIN_MAX_SIDECHAINS distinct sources reads silence. The
 * source is not delay-compensated relative to the reader.
 *
 * Automation: lanes with target SE_AUTO_BUILTIN name the device (device_id,
 * same track) and the parameter index (param_id); point values are PLAIN
 * values in the parameter's unit (no normalisation), interpolated like any
 * lane (geometric when `logarithmic`) and delivered as sample-offset
 * parameter events like plugin lanes. A lane whose index is out of range, or
 * whose device is not a built-in, is inactive. */
enum { SE_PLUGIN_BUILTIN = 3 };
enum { SE_AUTO_BUILTIN = 4 };

/* ---- switch automation (ABI v1 addition) ---------------------------------
 * SE_AUTO_MUTE:      the track's mute, 1 = muted (value >= 0.5), 0 = playing.
 *                    Replaces the static `mute` of the track (solo is
 *                    unchanged): the fader output and every send go silent
 *                    while muted. Master lanes ignore it.
 * SE_AUTO_DEVICE_ON: a device's power switch (`device_id`: an insert or the
 *                    instrument plugin of the same track), 1 = on (value >=
 *                    0.5), 0 = off. Replaces the device's static `enabled`;
 *                    evaluated at the start of every block (at the stage's
 *                    delay-compensated timeline frame), so a switch lands on
 *                    the first block boundary at or after its point. An off
 *                    insert is bypassed like a statically disabled one
 *                    (delay compensated by its latency).
 * SE_AUTO_FLAG_STEPPED (se_auto_lane_desc::flags): the lane holds each
 *   point's value until the next point (no interpolation, no curve):
 *   a.frame <= t < b.frame -> a.value. Set for switches and for discrete
 *   built-in parameters (choices). Other flag bits must be 0.
 * Channel strip (se_session_publish_v10), evaluated per sample like volume
 * at the track fader's timeline frame; each replaces the track's static
 * strip setting (a track needs no `strips` entry to be automated):
 * SE_AUTO_TRIM:  dB, clamped to -24..24, gain = 10^(dB/20).
 * SE_AUTO_WIDTH: 0..2 (clamped), 1 = unchanged.
 * SE_AUTO_MONO:  1 = folded to mono (value >= 0.5; stepped). */
enum { SE_AUTO_MUTE = 5, SE_AUTO_DEVICE_ON = 6, SE_AUTO_TRIM = 7, SE_AUTO_WIDTH = 8, SE_AUTO_MONO = 9 };
enum { SE_AUTO_FLAG_STEPPED = 1 };
#define SE_BUILTIN_STATE_MAGIC 0x31494253u /* "SBI1" as a little-endian uint32 */
#define SE_BUILTIN_MAX_SIDECHAINS 8

typedef struct se_builtin_state {
    uint32_t magic;               /* SE_BUILTIN_STATE_MAGIC */
    uint32_t value_count;         /* float32 values that follow the header */
    uint64_t sidechain_track_id;  /* 0 = no sidechain */
} se_builtin_state; /* 16 bytes, align 8; followed by value_count float32 */

/* One registered built-in processor (static data, valid for the process
 * lifetime). Readouts are values the processor publishes for visualisers
 * (for example input level, output level and gain reduction in dB).
 * Instruments (instrument != 0) go in a track's instrument slot
 * (instrument_plugin) and render the track's notes; sample_slots > 0 means
 * the device plays audio assets named in its state (see below). */
typedef struct se_builtin_desc {
    const char* id;
    const char* const* param_ids;   /* param_count entries, registry order */
    const char* const* readout_ids; /* readout_count entries */
    uint32_t param_count;
    uint32_t readout_count;
    const float* param_defaults; /* param_count entries: the processor's defaults */
    uint32_t sample_slots;
    uint8_t instrument;
    uint8_t _pad[3];
} se_builtin_desc; /* 48 bytes, align 8 */

/* Built-in devices that play audio files (samplers, drum pads) use the v2
 * state: the v1 header extended by the sample table. After the header come
 * value_count float32 values, then sample_count se_builtin_sample entries,
 * then slice_count se_builtin_slice entries (each sample's slices in table
 * order), then route_count se_mod_route entries (modulation sources, see
 * "modulation" below). Every asset_id names an asset loaded with se_asset_load_file; an
 * unknown asset leaves its slot empty. Slots must be < the device's
 * sample_slots and unique; slices start in [0, 1) and ascend. The engine
 * keeps a reference to every asset a live device uses, so releasing an
 * asset while a device plays it is safe. Validation (else
 * SE_ERR_INVALID_ARG): state_len == 32 + 4 * value_count + 16 *
 * sample_count + 16 * slice_count + 40 * route_count, slice counts add up,
 * values finite. */
#define SE_BUILTIN_STATE_MAGIC2 0x32494253u /* "SBI2" */
#define SE_BUILTIN_MAX_SLICES 128

typedef struct se_builtin_state_v2 {
    uint32_t magic;               /* SE_BUILTIN_STATE_MAGIC2 */
    uint32_t value_count;
    uint64_t sidechain_track_id;  /* 0 = no sidechain */
    uint32_t sample_count;
    uint32_t slice_count;         /* total over all samples */
    uint32_t route_count;         /* se_mod_route entries after the slices */
    uint32_t _pad;
} se_builtin_state_v2; /* 32 bytes, align 8 */

typedef struct se_builtin_sample {
    uint64_t asset_id;
    uint32_t slot;
    uint32_t slice_count; /* at most SE_BUILTIN_MAX_SLICES */
} se_builtin_sample; /* 16 bytes, align 8 */

typedef struct se_builtin_slice {
    float start;   /* fraction of the sample, 0 <= start < 1 */
    float gain_db; /* -60 .. 12 */
    float pan;     /* -1 .. 1 */
    float pitch;   /* semitones, -24 .. 24 */
} se_builtin_slice; /* 16 bytes, align 4 */

/* Every registered built-in processor. Any thread. */
const se_builtin_desc* se_builtin_registry(size_t* out_count);

/* Set one parameter of a live built-in device right away (plain value,
 * clamped by the processor; lock-free hand-over, applied at the next block).
 * The next publish overwrites it with the value in the device's state.
 * MAIN THREAD (like the other se_plugin_* calls). Unknown or non-built-in
 * device -> SE_ERR_NOT_FOUND; index out of range or value not finite ->
 * SE_ERR_INVALID_ARG. */
int32_t se_builtin_set_param(se_engine* engine, uint64_t device_id, uint32_t index, float value);

/* Latest readouts of a live built-in device (written by the audio thread
 * after every processed block; held while the device is not processed).
 * Writes up to `capacity`; *out_count = the device's readout count.
 * MAIN THREAD. Unknown or non-built-in device -> SE_ERR_NOT_FOUND. */
int32_t se_builtin_readouts(se_engine* engine, uint64_t device_id, float* out, size_t capacity, size_t* out_count);

/* Newest analysis frame of a live built-in device: bulk data a processor
 * publishes for visualisers (for example the Spectrum's band levels in dB;
 * the layout is the device's, see its processor). Written by the audio
 * thread whenever the processor completes a frame (lock-free triple
 * buffer). Copies up to `capacity` floats; *out_count = the frame's float
 * count (0 when the device publishes none or none yet); *out_seq = the
 * frame's publish number (0 = none yet; equal numbers = the same frame).
 * MAIN THREAD. Unknown or non-built-in device -> SE_ERR_NOT_FOUND. */
int32_t se_builtin_analysis(se_engine* engine, uint64_t device_id, float* out, size_t capacity, size_t* out_count,
                            uint64_t* out_seq);

/* ---- modulation (ABI v1 addition) -----------------------------------------
 * See docs/devices.md ("Modulation") and dstudio_core::modulation.
 *
 * A built-in device that is a modulation source (registry ids "lfo" and
 * "device-rack") lists the parameters it moves in the route table of its v2
 * state (se_builtin_state_v2): route_count se_mod_route records after the
 * slices (read with memcpy; no alignment). SE_BUILTIN_STATE_MAGIC blobs
 * carry no routes. Validation (else SE_ERR_INVALID_ARG): values and ranges
 * finite, kind <= SE_MOD_PLUGIN.
 *
 * Every SE_MOD_GRID_FRAMES frames of a block the engine evaluates each
 * enabled source of a track (its automation applied) and applies its routes
 * to targets on the same track (inserts or the instrument plugin; others are
 * inactive), in the target parameter's normalized 0..1 domain:
 *   n = (absolute route ? min + (max - min) * value : normalized base)
 *       + sum over additive routes of gain * (min + (max - min) * value)
 * clamped to 0..1 (the last absolute route wins). Source outputs: an LFO has
 * one (value = wave position 0..1, gain = Depth); a chain head has eight
 * macros (value = macro / 100, gain 1, routes absolute). Built-in targets:
 * base = the parameter's plain value (automation included), normalized with
 * the route's plain range and taper (log: ln(v / p_min) / ln(p_max / p_min);
 * linear: ((v - p_min) / (p_max - p_min))^p_skew) and converted back the same
 * way, then snapped to p_step. Plugin targets: base = the value the
 * parameter had when its first route appeared, or its automation lane, or
 * the last se_plugin_set_param; modulated values are sent as parameter
 * events and the base is restored when the last route goes away. */
#define SE_MOD_GRID_FRAMES 32u
enum { SE_MOD_BUILTIN = 0, SE_MOD_PLUGIN = 1 };

typedef struct se_mod_route {
    uint64_t target_device_id; /* an insert or the instrument plugin of the same track */
    uint32_t param_id;         /* SE_MOD_BUILTIN: parameter index; SE_MOD_PLUGIN: format id */
    uint8_t kind;              /* SE_MOD_* */
    uint8_t slot;              /* source output (macro index; 0 for an LFO) */
    uint8_t absolute;          /* != 0: sets the value (macros) instead of adding */
    uint8_t log;               /* SE_MOD_BUILTIN: logarithmic taper */
    float min;                 /* link range, normalized domain */
    float max;
    float p_min;               /* SE_MOD_BUILTIN: plain range, skew and step of the target */
    float p_max;
    float p_skew;
    float p_step;
} se_mod_route; /* 40 bytes, align 8 */

/* Set a plugin parameter from the host (a plugin card control), normalized
 * 0..1; delivered at the next processed block. A modulated parameter keeps
 * the value as its base. MAIN THREAD. Unknown or failed device, built-in
 * device or unknown parameter -> SE_ERR_NOT_FOUND; value not finite ->
 * SE_ERR_INVALID_ARG. */
int32_t se_plugin_set_param(se_engine* engine, uint64_t device_id, uint32_t param_id, float normalized);
/* Base value of a modulated plugin parameter (what its card shows).
 * SE_ERR_NOT_FOUND when the parameter is not modulated. MAIN THREAD. */
int32_t se_mod_base(se_engine* engine, uint64_t device_id, uint32_t param_id, float* out_normalized);

/* Play a note on a live built-in instrument right away (pads, auditions,
 * previews), on top of the notes of its track. velocity 1..127 = note on,
 * 0 = note off. Lock-free hand-over, applied at the start of the next
 * block; up to 256 events wait per device, more are dropped
 * (SE_ERR_STATE). MAIN THREAD. Unknown device, or one that is not a built-in
 * instrument -> SE_ERR_NOT_FOUND; pitch > 127 or velocity > 127 ->
 * SE_ERR_INVALID_ARG. */
int32_t se_builtin_note(se_engine* engine, uint64_t device_id, uint8_t pitch, uint8_t velocity);

/* ---- Preview (browser audition) voice -------------------------------------
 * One voice that plays a loaded asset from start_frame (clamped into the
 * asset; 0 = its start, used to resume after a stop), independent of the
 * transport, mixed into the master output after the master meters' track
 * sum (it shows on the master meters) and never into exports. A new play
 * replaces the sounding one with a short crossfade; the voice stops by
 * itself at the asset's end. gain: linear 0..2. rate: playback speed
 * 0.25..4 (1 = as recorded; varispeed, pitch follows). solo: while the
 * preview sounds, the arrangement is silent. Lock-free hand-over, applied
 * at the next block. MAIN THREAD. Unknown asset -> SE_ERR_NOT_FOUND; gain
 * or rate out of range or not finite -> SE_ERR_INVALID_ARG. */
int32_t se_preview_play(se_engine* engine, uint64_t asset_id, int64_t start_frame, float gain, float rate, uint8_t solo);
int32_t se_preview_stop(se_engine* engine); /* fades out; also releases held preview notes */
int32_t se_preview_set(se_engine* engine, float gain, float rate, uint8_t solo);
/* Preview a note through a track's instrument (piano-roll keys, drawn and
 * moved notes): device_id is the track's instrument device, built-in or
 * plugin. velocity 1..127 = note on, 0 = note off. The note plays at the
 * start of the next block on top of the track's own notes, whether the
 * transport runs or not, and goes through the track's chain and mixer like
 * them (never into exports, which render their own session). The engine
 * remembers the held notes: se_preview_stop releases them, and
 * se_preview_info.notes counts them. Lock-free hand-over (the live note
 * queue the device shares with se_builtin_note, 256 events). MAIN THREAD.
 * Unknown device, or one that plays no notes (an effect) -> SE_ERR_NOT_FOUND;
 * pitch or velocity > 127 -> SE_ERR_INVALID_ARG; queue full -> SE_ERR_STATE. */
int32_t se_preview_note(se_engine* engine, uint64_t device_id, uint8_t pitch, uint8_t velocity);

typedef struct se_preview_info {
    uint8_t playing;     /* 1 while a voice sounds (not counting a fade-out) */
    uint8_t notes;       /* preview notes held (se_preview_note), at most 255 */
    uint8_t reserved[6];
    uint64_t asset_id;   /* asset of the sounding voice, 0 when none */
    int64_t position;    /* frames into the asset */
    int64_t frames;      /* asset length in frames */
    float peak_l;        /* preview-only peaks since the previous call */
    float peak_r;
} se_preview_info; /* 40 bytes, align 8 */
int32_t se_preview_get(se_engine* engine, se_preview_info* out);

/* ---- return tracks, sends and output routing (ABI v1 addition) ----------
 * See engine/README.md ("Return tracks and sends") and specs/002.
 *
 * se_session_publish_v9 is se_session_publish_v8 plus routing. A track
 * listed in `routes` (by track_id, once) with is_return != 0 is a return
 * track: its source is the sum of what other tracks send or route to it
 * (its "bus"); then its insert chain, volume, pan, meters and automation
 * work like any track's, and it plays into the master. output_track_id names
 * the return a track plays into instead of the master (0 = master). A send
 * copies a track's signal into a return at `gain` (linear): pre_fader != 0
 * taps after the insert chain, before volume and pan; otherwise after them
 * (the same signal the track's output carries). Tracks not listed play into
 * the master and are not returns.
 *
 * Mute and solo: a muted track plays and sends nothing. With ordinary
 * tracks soloed, only they play and send, and the returns keep playing
 * (solo-safe). With returns soloed, only they play, still fed by every
 * unmuted track that is not soloed out by an ordinary solo. A track routed
 * into a return follows the rule for sends.
 *
 * Delay compensation (se_pdc_*): a return receives its inputs aligned at the
 * latest of their chains' latencies and adds its own; every input of the
 * master and of each return arrives aligned. The latency report lists
 * return tracks like other tracks (their own chain latency).
 *
 * Validation (else SE_ERR_INVALID_ARG): arrays non-NULL when counted; route
 * and send track ids known; a track routed at most once; at most
 * SE_MAX_RETURNS returns; a return has no clips, notes, instrument, input,
 * arm or monitoring and no output other than the master; an output or send
 * target is a return track other than the track itself; a track sends to a
 * return at most once; gain finite in 0..4.
 *
 * Sends between returns (a later addition, for group tracks, which the app
 * publishes as returns): a return may send into other returns as long as the
 * sends between returns form no loop. Its sends follow the rule for its main
 * output under mute and solo, and delay compensation counts the return's own
 * arrival and chain before the return it feeds. */
#define SE_MAX_RETURNS 16

typedef struct se_track_route {
    uint64_t track_id;
    uint64_t output_track_id; /* a return track, 0 = master */
    uint8_t is_return;
    uint8_t _pad[7];
} se_track_route; /* 24 bytes, align 8 */

typedef struct se_send_desc {
    uint64_t track_id;        /* the sending track */
    uint64_t return_track_id; /* the return it feeds */
    float gain;               /* linear, 0..4 */
    uint8_t pre_fader;
    uint8_t _pad[3];
} se_send_desc; /* 24 bytes, align 8 */

typedef struct se_session_desc_v9 {
    se_session_desc_v8 v8;
    const se_track_route* routes;
    uint32_t route_count;
    uint32_t send_count;
    const se_send_desc* sends;
} se_session_desc_v9; /* 88 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v9(se_engine* engine, const se_session_desc_v9* session);

/* ---- channel strip: trim, width and mono (ABI v1 addition) ---------------
 * See specs/004-channel-strip-trim-width-mono.
 *
 * se_session_publish_v10 is se_session_publish_v9 plus per-track strip
 * settings. A track listed in `strips` (by track_id, once) has its signal
 * changed after its insert chain and before pre-fader sends, volume and pan:
 * scaled by `trim` (linear), then its stereo width set by mid/side: the side
 * signal (L - R) / 2 is scaled by `width` (0 = mono, 1 = unchanged, 2 =
 * doubled) around the mid (L + R) / 2. mono != 0 folds the track to mono
 * (the same as width 0) whatever `width` says. Tracks not listed, and listed
 * tracks with trim 1, width 1 and mono 0, are unchanged (and cost nothing).
 * Return tracks take strip settings like any track.
 *
 * Validation (else SE_ERR_INVALID_ARG): strips non-NULL when counted; track
 * ids known and listed at most once; trim finite in 0..16; width finite in
 * 0..2. */
typedef struct se_track_strip {
    uint64_t track_id;
    float trim;  /* linear, 0..16 */
    float width; /* 0..2, 1 = unchanged */
    uint8_t mono;
    uint8_t _pad[7];
} se_track_strip; /* 24 bytes, align 8 */

typedef struct se_session_desc_v10 {
    se_session_desc_v9 v9;
    const se_track_strip* strips;
    uint32_t strip_count;
    uint32_t _pad;
} se_session_desc_v10; /* 104 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v10(se_engine* engine, const se_session_desc_v10* session);

/* ---- audio clip fades (ABI v1 addition) ----------------------------------
 * se_session_publish_v11 is se_session_publish_v10 plus clip fades. A clip
 * listed in `fades` (by clip_id, which must name a clip of the session, once)
 * fades in over its first `fade_in` timeline frames and out over its last
 * `fade_out`: at clip-relative frame r of a clip whose played range is
 * `len` frames long (after the clamps of se_session_publish_v8), its output
 * is scaled by min(1, r / fade_in) and by min(1, (len - r) / fade_out), a
 * linear ramp from silence (a 0-length fade is no fade). Fades follow the
 * timeline, so a looped or warped clip fades where it plays; each fade is
 * clamped to the clip, and the fade-out to what the fade-in leaves. Clips
 * not listed, and listed clips with both fades 0, are unchanged (and cost
 * nothing); blocks outside a clip's fades cost nothing either. Rendering
 * never allocates or locks.
 *
 * Validation (else SE_ERR_INVALID_ARG): fades non-NULL when counted; clip ids
 * known and listed at most once; fade_in and fade_out >= 0. */
typedef struct se_clip_fade_desc {
    uint64_t clip_id;
    int64_t fade_in;  /* timeline frames from the clip start */
    int64_t fade_out; /* timeline frames back from the clip end */
} se_clip_fade_desc; /* 24 bytes, align 8 */

typedef struct se_session_desc_v11 {
    se_session_desc_v10 v10;
    const se_clip_fade_desc* fades;
    uint32_t fade_count;
    uint32_t _pad;
} se_session_desc_v11; /* 120 bytes, align 8 */

/* MAIN THREAD (like se_session_publish_v5). */
int32_t se_session_publish_v11(se_engine* engine, const se_session_desc_v11* session);

/* ---- MIDI input (specs/003-midi-input-and-recording) ----------------------
 * The engine reads MIDI input ports (Core MIDI on macOS, the Windows
 * multimedia MIDI API, the ALSA sequencer on Linux; none elsewhere) and
 * plays what arrives on instrument tracks, from the next audio block:
 *  - every instrument track whose `arm` or `monitor` is set, or, when no
 *    instrument track has either,
 *  - the instrument track set with se_midi_set_focus (the selected track).
 * Messages are folded onto channel 1. Notes go to the track's instrument
 * (the built-in synth, a built-in instrument or an instrument plugin);
 * controllers, pitch bend and aftertouch go to instruments that take raw
 * MIDI. A track that stops receiving (routing change, port closed) gets
 * note-offs for the keys it still holds. Up to 256 messages are delivered
 * per audio callback; each port queues up to 1024 more (then drops and
 * counts them).
 * Nothing is opened until se_midi_inputs_open names ports; the MIDI system
 * itself is first touched by se_midi_inputs_list / se_midi_inputs_open. */
typedef struct se_midi_input_info {
    char id[SE_DEVICE_STRING_MAX];   /* stable id, UTF-8 (what se_midi_inputs_open takes) */
    char name[SE_DEVICE_STRING_MAX]; /* display name, UTF-8 (truncated) */
    uint64_t events;                 /* messages received since it was opened (activity) */
    uint8_t open;                    /* 1 while open */
    uint8_t _pad[7];
} se_midi_input_info; /* 528 bytes, align 8 */

/* Lists the MIDI input ports present now (rescans). Writes up to `capacity`;
 * *out_count receives the total (which may exceed capacity). No MIDI system
 * -> SE_OK with 0 ports. MAIN THREAD. */
int32_t se_midi_inputs_list(se_engine* engine, se_midi_input_info* out, size_t capacity, size_t* out_count);
/* Opens exactly the ports in `ids` that exist and closes every other open
 * port (count 0 closes all). Ids that are not present are ignored. At most
 * 16 ports are open at once. A port that fails to open -> SE_ERR_DEVICE
 * (se_last_error names it; the others are still opened). MAIN THREAD. */
int32_t se_midi_inputs_open(se_engine* engine, const char* const* ids, size_t count);
/* Delivers one channel message (status 0x80..0xEF) as if a port had sent it
 * (tests, on-screen keyboards). Not a channel message -> SE_ERR_INVALID_ARG;
 * queue full -> SE_ERR_STATE. MAIN THREAD. */
int32_t se_midi_inject(se_engine* engine, uint8_t status, uint8_t data1, uint8_t data2);
/* The instrument track that plays MIDI input while no instrument track is
 * armed or monitoring (the selected track); 0 = none. Any thread. */
int32_t se_midi_set_focus(se_engine* engine, uint64_t track_id);

/* Capture (MIDI recording). While enabled and the transport plays, every
 * note-on and note-off delivered to instruments is written to a ring of
 * 4096 events with the timeline frame it was heard at: the block position
 * minus the output latency and delay compensation, folded back into the
 * loop after a loop wrap. Enabling empties the ring. The app reads it with
 * se_midi_capture_read (one reader). MAIN THREAD. */
typedef struct se_midi_captured {
    int64_t frame;   /* timeline frame (engine rate); may be < 0 near the start */
    uint8_t status;  /* 0x90 note-on or 0x80 note-off (channel 1) */
    uint8_t data1;   /* pitch */
    uint8_t data2;   /* velocity (note-off: release velocity) */
    uint8_t _pad[5];
} se_midi_captured; /* 16 bytes, align 8 */
int32_t se_midi_capture_enable(se_engine* engine, uint8_t enabled);
/* Pops up to `capacity` captured events in order; *out_count receives how
 * many were written. MAIN THREAD. */
int32_t se_midi_capture_read(se_engine* engine, se_midi_captured* out, size_t capacity, size_t* out_count);
/* *out_wrapped = 1 when the transport wrapped at the loop end while the
 * latest capture pass ran (also after it was disabled, until the next
 * enable), else 0. Seeks never count. Lets the app tell a loop wrap from a
 * seek back. Any thread. */
int32_t se_midi_capture_wrapped(se_engine* engine, uint8_t* out_wrapped);

typedef struct se_midi_stats {
    uint64_t received;        /* messages received from ports and se_midi_inject */
    uint64_t delivered;       /* messages handed to the audio thread's routing */
    uint64_t dropped;         /* messages lost to a full port queue */
    uint64_t capture_dropped; /* captured events lost to a full capture ring */
} se_midi_stats; /* 32 bytes, align 8 */
int32_t se_midi_stats_get(se_engine* engine, se_midi_stats* out);

#ifdef __cplusplus
}
#endif
#endif /* DSTUDIO_ENGINE_H */
