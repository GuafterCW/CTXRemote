/**
 * This computer's microphone for the host: Opus packets of 20 ms (48 kHz
 * mono), encoded with WebCodecs. An AudioContext at 48 kHz puts the
 * microphone at the rate Opus wants, whatever the device delivers.
 */

// MediaStreamTrackProcessor is in Chromium (WebView2) but not in TypeScript's DOM types yet.
declare class MediaStreamTrackProcessor<T> {
  constructor(init: { track: MediaStreamTrack });
  readonly readable: ReadableStream<T>;
}

export class MicSender {
  private stream: MediaStream | null = null;
  private context: AudioContext | null = null;
  private encoder: AudioEncoder | null = null;
  private stopped = false;

  static supported(): boolean {
    return (
      typeof AudioEncoder !== "undefined" &&
      typeof MediaStreamTrackProcessor !== "undefined" &&
      !!navigator.mediaDevices?.getUserMedia
    );
  }

  /** Asks for the microphone and starts sending; throws if it is refused. */
  async start(onPacket: (packet: Uint8Array) => void) {
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1 },
    });
    this.context = new AudioContext({ sampleRate: 48_000 });
    const source = this.context.createMediaStreamSource(this.stream);
    const target = this.context.createMediaStreamDestination();
    target.channelCount = 1;
    source.connect(target);

    this.encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        onPacket(data);
      },
      error: () => this.stop(),
    });
    this.encoder.configure({ codec: "opus", sampleRate: 48_000, numberOfChannels: 1, bitrate: 32_000 });

    const track = target.stream.getAudioTracks()[0];
    const reader = new MediaStreamTrackProcessor<AudioData>({ track }).readable.getReader();
    (async () => {
      while (!this.stopped) {
        const { value, done } = await reader.read();
        if (done || !value) break;
        if (this.encoder?.state === "configured") this.encoder.encode(value);
        value.close();
      }
    })().catch(() => this.stop());
  }

  stop() {
    this.stopped = true;
    this.stream?.getTracks().forEach((t) => t.stop());
    this.context?.close().catch(() => {});
    try {
      this.encoder?.close();
    } catch {
      // Already closed.
    }
    this.stream = null;
    this.context = null;
    this.encoder = null;
  }
}
