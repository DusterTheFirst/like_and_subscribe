/// <reference types="gapi" />
/// <reference types="gapi.auth2" />
/// <reference types="gapi.youtube" />

import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

const app = mount(App, {
  target: document.getElementById("app")!,
});

export default app;
