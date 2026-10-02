/**
 * Decodes the host's H.264 stream with WebCodecs and paints it onto a canvas.
 * Packets arrive in Annex B form, which VideoDecoder accepts when no
 * `description` is configured.
 */
export class Player {
  private decoder: VideoDecoder | null = null;
  private ctx: CanvasRenderingContext2D;
  private waitingForKeyframe = true;
  private size = { width: 0, height: 0 };
  private timestamp = 0;

  constructor(
    private canvas: HTMLCanvasElement,
    private requestKeyframe: () => void,
    private onFirstFrame: () => void,
  ) {
    this.ctx = canvas.getContext("2d", { alpha: false, desynchronized: true })!;
  }

  push(keyframe: boolean, width: number, height: number, data: Uint8Array) {
    if (width !== this.size.width || height !== this.size.height) {
      this.size = { width, height };
      this.reset();
    }
    if (this.waitingForKeyframe) {
      if (!keyframe) return;
      this.waitingForKeyframe = false;
    }
    const decoder = this.decoder ?? this.createDecoder();
    // A backed-up decoder means we fall behind real time; skip to the next keyframe.
    if (decoder.decodeQueueSize > 4 && !keyframe) {
      this.reset();
      this.requestKeyframe();
      return;
    }
    try {
      decoder.decode(
        new EncodedVideoChunk({ type: keyframe ? "key" : "delta", timestamp: this.timestamp, data }),
      );
      this.timestamp += 33_333;
    } catch {
      this.recover();
    }
  }

  close() {
    this.decoder?.close();
    this.decoder = null;
  }

  private createDecoder(): VideoDecoder {
    let first = true;
    const decoder = new VideoDecoder({
      output: (frame) => {
        if (this.canvas.width !== frame.displayWidth || this.canvas.height !== frame.displayHeight) {
          this.canvas.width = frame.displayWidth;
          this.canvas.height = frame.displayHeight;
        }
        this.ctx.drawImage(frame, 0, 0);
        frame.close();
        if (first) {
          first = false;
          this.onFirstFrame();
        }
      },
      error: () => this.recover(),
    });
    decoder.configure({
      // Level 5.1 covers everything up to 4K; the stream itself is constrained baseline.
      codec: "avc1.42E033",
      codedWidth: this.size.width,
      codedHeight: this.size.height,
      optimizeForLatency: true,
    });
    this.decoder = decoder;
    return decoder;
  }

  private reset() {
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
    this.waitingForKeyframe = true;
  }

  private recover() {
    this.reset();
    this.requestKeyframe();
  }
}
