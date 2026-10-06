import { mount } from "svelte";
import "./styles.css";
import Home from "./Home.svelte";
import Quick from "./Quick.svelte";
import Files from "./Files.svelte";
import Session from "./Session.svelte";
import { api } from "./lib/api";
import { MOBILE } from "./lib/platform";

const match = location.hash.match(/^#\/session\/(\d+)$/);
const target = document.getElementById("app")!;

const files = location.hash.match(/^#\/files\/(\d+)$/);

if (files) document.body.classList.add("files");
// Phones show sessions and files in the one window: Android's back button
// changes the address, and the page follows. Leaving a session's pages ends
// it; going between its picture and its files does not.
if (MOBILE) {
  document.body.classList.add("mobile");
  const sessionOf = (url: string) => new URL(url).hash.match(/^#\/(?:session|files)\/(\d+)$/)?.[1];
  window.addEventListener("hashchange", async (e) => {
    const left = sessionOf(e.oldURL);
    if (left && left !== sessionOf(e.newURL)) await api.disconnect(Number(left)).catch(() => {});
    location.reload();
  });
}
if (match) document.body.classList.add("session");

export default files
  ? mount(Files, { target, props: { session: Number(files[1]) } })
  : match
  ? mount(Session, { target, props: { session: Number(match[1]) } })
  : location.hash === "#/quick"
    ? mount(Quick, { target })
    : mount(Home, { target });
