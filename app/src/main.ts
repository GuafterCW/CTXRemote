import { mount } from "svelte";
import "./styles.css";
import Home from "./Home.svelte";
import Session from "./Session.svelte";

const match = location.hash.match(/^#\/session\/(\d+)$/);
const target = document.getElementById("app")!;

if (match) document.body.classList.add("session");

export default match
  ? mount(Session, { target, props: { session: Number(match[1]) } })
  : mount(Home, { target });
