<script lang="ts" module>
  // 20×20 stroke icons drawn for this app; 1.5px strokes match the type weight.
  const paths = {
    copy: "M7 7V4.5A1.5 1.5 0 0 1 8.5 3h7A1.5 1.5 0 0 1 17 4.5v7a1.5 1.5 0 0 1-1.5 1.5H13M4.5 7h7A1.5 1.5 0 0 1 13 8.5v7a1.5 1.5 0 0 1-1.5 1.5h-7A1.5 1.5 0 0 1 3 15.5v-7A1.5 1.5 0 0 1 4.5 7Z",
    check: "M4 10.5 8 14.5 16 5.5",
    refresh: "M16 10a6 6 0 1 1-1.76-4.24M16 3.5V6.5H13",
    eye: "M2.5 10S5.5 4.5 10 4.5 17.5 10 17.5 10 14.5 15.5 10 15.5 2.5 10 2.5 10ZM10 12.25A2.25 2.25 0 1 0 10 7.75a2.25 2.25 0 0 0 0 4.5Z",
    eyeOff: "M3 3l14 14M8.4 8.5A2.25 2.25 0 0 0 11.5 11.6M6.1 5.6C3.8 7 2.5 10 2.5 10s3 5.5 7.5 5.5c1.4 0 2.6-.5 3.7-1.2M9 4.6c.3 0 .7-.1 1-.1 4.5 0 7.5 5.5 7.5 5.5s-.6 1.1-1.7 2.4",
    sliders: "M4 6h7M15 6h1M4 14h1M9 14h7M13 4v4M7 12v4",
    arrowRight: "M4 10h12M11 5l5 5-5 5",
    arrowLeft: "M16 10H4M9 5l-5 5 5 5",
    close: "M5 5l10 10M15 5 5 15",
    volume: "M3.5 8v4h3l4 3.5v-11L6.5 8h-3ZM13.5 7.5a3.5 3.5 0 0 1 0 5M15.5 5a7 7 0 0 1 0 10",
    volumeOff: "M3.5 8v4h3l4 3.5v-11L6.5 8h-3ZM13.5 8l4 4M17.5 8l-4 4",
    monitor: "M3.5 4.5h13a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1ZM7 17.5h6M10 14.5v3",
    expand: "M3.5 7.5v-4h4M12.5 3.5h4v4M16.5 12.5v4h-4M7.5 16.5h-4v-4",
    shrink: "M7.5 3.5v4h-4M16.5 7.5h-4v-4M12.5 16.5v-4h4M3.5 12.5h4v4",
    power: "M10 3v6.5M6 5.5a6 6 0 1 0 8 0",
    keyboard: "M3.5 5.5h13a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1ZM6 8.5h.01M9 8.5h.01M12 8.5h.01M15 8.5h.01M6.5 11.5h7",
    chevron: "M8 5l5 5-5 5",
    pencil: "M12.5 4.5l3 3M4 16l.8-3.6L13.3 3.9a1.4 1.4 0 0 1 2 0l.8.8a1.4 1.4 0 0 1 0 2l-8.5 8.5L4 16Z",
    plus: "M10 4v12M4 10h12",
    folder: "M2.5 5.5a1 1 0 0 1 1-1h4l1.7 2h7.3a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1v-9Z",
    file: "M5.5 2.5h6l3.5 3.5v10.5a1 1 0 0 1-1 1h-8.5a1 1 0 0 1-1-1v-13a1 1 0 0 1 1-1ZM11.5 2.5V6H15",
    drive: "M3 11.5l2-6.5h10l2 6.5v3.5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-3.5ZM3 11.5h14M14 13.75h.01",
    upload: "M10 13V4M6 8l4-4 4 4M4 15.5h12",
    download: "M10 4v9M6 9l4 4 4-4M4 15.5h12",
    trash: "M4 6h12M8 6V4h4v2M5.5 6l.7 10a1 1 0 0 0 1 .9h5.6a1 1 0 0 0 1-.9L14.5 6M8.5 9v5M11.5 9v5",
    folderPlus: "M2.5 5.5a1 1 0 0 1 1-1h4l1.7 2h7.3a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-13a1 1 0 0 1-1-1v-9ZM10 9.5v4M8 11.5h4",
    arrowUp: "M10 16V4M5 9l5-5 5 5",
    chat: "M4.5 3.5h11a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H9.5L6 16.5v-3H4.5a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1ZM7 7.5h6M7 10h3.5",
    home: "M3 9.5 10 3.5l7 6M5 8.5v7.5h3.5v-4.5h3V16H15V8.5",
    user: "M10 10a3.25 3.25 0 1 0 0-6.5 3.25 3.25 0 0 0 0 6.5ZM3.5 16.5a6.5 6.5 0 0 1 13 0",
    history: "M10 17.5a7.5 7.5 0 1 0-7.2-9.5M2.5 3.5V8H7M10 6v4l2.5 1.5",
  } as const;

  export type IconName = keyof typeof paths;
</script>

<script lang="ts">
  let { name, size = 18 }: { name: IconName; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 20 20"
  fill="none"
  stroke="currentColor"
  stroke-width="1.5"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  <path d={paths[name]} />
</svg>
