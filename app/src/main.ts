import { mount } from "svelte";
import "./styles.css";
import Home from "./Home.svelte";
import Quick from "./Quick.svelte";
import Files from "./Files.svelte";
import Session from "./Session.svelte";

const match = location.hash.match(/^#\/session\/(\d+)$/);
const target = document.getElementById("app")!;

const files = location.hash.match(/^#\/files\/(\d+)$/);

if (files) document.body.classList.add("files");
// Phones show sessions and files in the one window: Android's back button
// changes the address, and the page follows.
if (/Android|iPhone|iPad/.test(navigator.userAgent)) {
  document.body.classList.add("mobile");
  window.addEventListener("hashchange", () => location.reload());
}
if (match) document.body.classList.add("session");

export default files
  ? mount(Files, { target, props: { session: Number(files[1]) } })
  : match
  ? mount(Session, { target, props: { session: Number(match[1]) } })
  : location.hash === "#/quick"
    ? mount(Quick, { target })
    : mount(Home, { target });
