/**
 * Plays the host's sound: Opus packets of 20 ms (48 kHz stereo), decoded
 * with WebCodecs and scheduled back to back on an AudioContext.
 *
 * A small lead (LEAD) absorbs jitter. When the queue grows beyond MAX_LEAD,
 * e.g. after the network stalled, it starts over at the lead instead of
 * playing everything late.
 */
const LEAD = 0.06;
const MAX_LEAD = 0.25;
const FRAME_US = 20_000;

export class SoundPlayer {
  private context: AudioContext | null = null;
  private gain: GainNode | null = null;
  private decoder: AudioDecoder | null = null;
  private next = 0;
  private timestamp = 0;
  private muted = false;

  /** Whether this webview can decode Opus at all. */
  static supported(): boolean {
    return typeof AudioDecoder !== "undefined" && typeof AudioContext !== "undefined";
  }

  push(packet: Uint8Array) {
    if (!SoundPlayer.supported()) return;
    const decoder = this.decoder ?? this.open();
    if (decoder.state !== "configured") return;
    decoder.decode(new EncodedAudioChunk({ type: "key", timestamp: this.timestamp, data: packet }));
    this.timestamp += FRAME_US;
  }

  setMuted(muted: boolean) {
    this.muted = muted;
    if (this.gain) this.gain.gain.value = muted ? 0 : 1;
  }

  /** Browsers start audio only after a user gesture; call this from one. */
  resume() {
    if (this.context?.state === "suspended") this.context.resume().catch(() => {});
  }

  close() {
    try {
      this.decoder?.close();
    } catch {
      // Already closed.
    }
    this.context?.close().catch(() => {});
    this.decoder = null;
    this.context = null;
    this.gain = null;
  }

  private open(): AudioDecoder {
    const context = new AudioContext({ sampleRate: 48_000, latencyHint: "interactive" });
    const gain = context.createGain();
    gain.gain.value = this.muted ? 0 : 1;
    gain.connect(context.destination);
    this.context = context;
    this.gain = gain;
    const decoder = new AudioDecoder({
      output: (data) => this.play(data),
      // A broken packet: start a fresh decoder with the next one.
      error: () => {
        this.decoder = null;
      },
    });
    decoder.configure({ codec: "opus", sampleRate: 48_000, numberOfChannels: 2 });
    this.decoder = decoder;
    return decoder;
  }

  private play(data: AudioData) {
    const context = this.context;
    if (!context || !this.gain) {
      data.close();
      return;
    }
    const buffer = context.createBuffer(data.numberOfChannels, data.numberOfFrames, data.sampleRate);
    for (let channel = 0; channel < data.numberOfChannels; channel++) {
      const samples = new Float32Array(data.numberOfFrames);
      data.copyTo(samples, { planeIndex: channel, format: "f32-planar" });
      buffer.copyToChannel(samples, channel);
    }
    data.close();
    const now = context.currentTime;
    if (this.next < now || this.next - now > MAX_LEAD) this.next = now + LEAD;
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.gain);
    source.start(this.next);
    this.next += buffer.duration;
  }
}
