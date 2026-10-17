// Sony Connect UI — vanilla JS on top of window.__TAURI__ (withGlobalTauri).
(() => {
  "use strict";

  const T = window.__TAURI__;
  const invoke = T ? T.core.invoke : async () => { throw new Error("Tauri API unavailable"); };
  const listen = T ? T.event.listen : async () => {};

  const $ = (id) => document.getElementById(id);
  const el = {
    name: $("device-name"), status: $("status"), statusText: $("status-text"),
    batteryCard: $("battery-card"), battery: $("battery"), batteryFill: $("battery-fill"),
    batteryLevel: $("battery-level"), batterySub: $("battery-sub"), reconnect: $("btn-reconnect"),
    volumeRow: $("volume-row"), volume: $("volume"), volumeValue: $("volume-value"), volIcon: $("vol-icon"),
    ancCard: $("anc-card"), anc: $("anc"), ambient: $("ambient"), ambientLevel: $("ambient-level"),
    ambientValue: $("ambient-value"), focus: $("focus"), focusRow: $("focus-row"),
    ultCard: $("ult-card"), ult: $("ult"), ultDesc: $("ult-desc"),
    model: $("model"), firmware: $("firmware"), backendTag: $("backend-tag"),
    toast: $("toast"), popover: $("devices-popover"), deviceList: $("device-list"),
    btnDevices: $("btn-devices"), btnAuto: $("btn-auto"),
  };

  const ULT_DESC = { off: "Standard sound", ult1: "Deep bass", ult2: "Powerful, punchy bass" };

  let state = null;       // last DeviceState
  let connection = null;  // last Connection
  // While the user drags a slider, ignore incoming values for it.
  const dragging = { volume: false, ambient: false };

  // ------------------------------------------------------------ helpers

  function setPct(range) {
    const min = +range.min, max = +range.max;
    range.style.setProperty("--pct", ((+range.value - min) / (max - min)) * 100 + "%");
  }

  function setVolIcon(v) {
    el.volIcon.dataset.level = v <= 0 ? "0" : v < 50 ? "1" : "2";
    // Icon sits on the filled part once the fill passes it (~12% of the track).
    el.volIcon.classList.toggle("dim", v < 12);
  }

  function setUnavailable(node, unavailable) {
    node.classList.toggle("unavailable", unavailable);
    node.querySelectorAll("input, button").forEach((n) => (n.disabled = unavailable));
  }

  function selectRadio(group, value) {
    let index = -1;
    group.querySelectorAll("button").forEach((b, i) => {
      const on = b.dataset.value === value;
      b.setAttribute("aria-checked", String(on));
      if (on) index = i;
    });
    if (group.classList.contains("segmented")) {
      group.style.setProperty("--i", Math.max(index, 0));
      group.classList.toggle("none", index < 0);
    }
  }

  function setAnc(mode) {
    document.body.dataset.anc = mode || "none";
    el.ambient.classList.toggle("open", mode === "ambient");
  }

  function setUlt(mode) {
    document.body.dataset.ult = mode || "off";
    el.ultDesc.textContent = mode ? ULT_DESC[mode] || "" : "Unavailable";
  }

  // Roll the battery number towards its new value.
  let shownLevel = null, rollRaf = 0;
  function countTo(target) {
    cancelAnimationFrame(rollRaf);
    const from = shownLevel == null ? Math.max(0, target - 25) : shownLevel;
    if (from === target) { el.batteryLevel.textContent = target + "%"; return; }
    const t0 = performance.now(), dur = 700;
    const step = (now) => {
      const p = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - p, 3);
      shownLevel = Math.round(from + (target - from) * e);
      el.batteryLevel.textContent = shownLevel + "%";
      if (p < 1) rollRaf = requestAnimationFrame(step);
    };
    rollRaf = requestAnimationFrame(step);
  }

  let toastTimer;
  function toast(msg) {
    el.toast.textContent = msg;
    el.toast.classList.remove("hidden");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => el.toast.classList.add("hidden"), 3500);
  }

  async function call(cmd, args) {
    try {
      return await invoke(cmd, args);
    } catch (e) {
      toast(String(e));
      throw e;
    }
  }

  function debounce(fn, ms) {
    let t;
    return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
  }

  // ------------------------------------------------------------ rendering

  function renderConnection(c) {
    connection = c;
    if (!c) return;
    const s = c.status;
    const online = s === "connected";
    el.status.className = "chip " + s;
    el.statusText.textContent = online ? "Connected" : s === "connecting" ? "Connecting…" : "Not connected";
    el.status.title = c.error || "";
    if (c.device) el.name.textContent = c.device.name;
    else if (!state) el.name.textContent = "Not Connected";
    document.body.classList.toggle("offline", !online);
    el.reconnect.hidden = s !== "disconnected";
    el.backendTag.textContent = c.backend === "mock" ? " · demo mode" : "";
    renderSubline();
  }

  function renderSubline() {
    const b = state && state.battery;
    const s = connection && connection.status;
    let text;
    if (s === "connecting") text = "Connecting to your headphones…";
    else if (s !== "connected") {
      text = "Turn on your headphones to connect";
      if (connection && connection.retry_in != null) text += ` · retrying in ${connection.retry_in}s`;
    } else if (b && b.charging) text = "Charging";
    else if (b && b.level <= 15) text = "Low battery";
    else text = "";
    el.batterySub.textContent = text;
  }

  function renderState(s) {
    state = s;
    s = s || {};

    // Battery
    const b = s.battery;
    if (b && typeof b.level === "number") {
      const lvl = Math.max(0, Math.min(100, b.level));
      el.batteryCard.classList.remove("unavailable");
      countTo(lvl);
      el.batteryFill.style.width = lvl + "%";
      el.batteryFill.classList.toggle("low", lvl <= 15);
      el.battery.classList.toggle("charging", !!b.charging);
    } else {
      el.batteryCard.classList.add("unavailable");
      el.batteryLevel.textContent = "—";
      shownLevel = null;
      el.batteryFill.style.width = "0";
      el.battery.classList.remove("charging");
    }
    renderSubline();

    // Volume
    const hasVol = typeof s.volume === "number";
    setUnavailable(el.volumeRow, !hasVol);
    if (hasVol && !dragging.volume) el.volume.value = s.volume;
    el.volumeValue.textContent = hasVol ? el.volume.value + "%" : "Unavailable";
    setPct(el.volume);
    setVolIcon(+el.volume.value);

    // Noise control
    const hasAnc = typeof s.anc === "string";
    setUnavailable(el.ancCard, !hasAnc);
    selectRadio(el.anc, hasAnc ? s.anc : null);
    setAnc(hasAnc ? s.anc : null);
    const hasLevel = typeof s.ambient_level === "number";
    el.ambientLevel.disabled = !hasLevel;
    if (hasLevel && !dragging.ambient) el.ambientLevel.value = s.ambient_level;
    el.ambientValue.textContent = hasLevel ? el.ambientLevel.value : "—";
    setPct(el.ambientLevel);
    const hasFocus = typeof s.focus_on_voice === "boolean";
    el.focusRow.classList.toggle("unavailable", !hasFocus);
    el.focus.disabled = !hasFocus;
    el.focus.checked = hasFocus && s.focus_on_voice;

    // ULT
    const hasUlt = typeof s.ult === "string";
    setUnavailable(el.ultCard, !hasUlt);
    selectRadio(el.ult, hasUlt ? s.ult : null);
    setUlt(hasUlt ? s.ult : null);

    // Footer
    el.model.textContent = s.model || "Sony headphones";
    el.firmware.textContent = s.firmware ? "Firmware " + s.firmware : "Firmware —";
  }

  // ------------------------------------------------------------ controls

  const sendVolume = debounce((v) => call("set_volume", { volume: v }).catch(() => {}), 120);
  el.volume.addEventListener("input", () => {
    dragging.volume = true;
    el.volumeValue.textContent = el.volume.value + "%";
    setPct(el.volume);
    setVolIcon(+el.volume.value);
    sendVolume(+el.volume.value);
  });
  el.volume.addEventListener("change", () => { dragging.volume = false; });

  function sendAnc(mode) {
    return call("set_anc", {
      mode,
      ambientLevel: +el.ambientLevel.value,
      focusOnVoice: el.focus.checked,
    }).catch(() => renderState(state));
  }

  el.anc.addEventListener("click", (e) => {
    const btn = e.target.closest("button");
    if (!btn || btn.disabled) return;
    selectRadio(el.anc, btn.dataset.value);
    setAnc(btn.dataset.value);
    sendAnc(btn.dataset.value);
  });

  const sendAmbient = debounce(() => sendAnc("ambient"), 150);
  el.ambientLevel.addEventListener("input", () => {
    dragging.ambient = true;
    el.ambientValue.textContent = el.ambientLevel.value;
    setPct(el.ambientLevel);
    sendAmbient();
  });
  el.ambientLevel.addEventListener("change", () => { dragging.ambient = false; });
  el.focus.addEventListener("change", () => sendAnc("ambient"));

  el.ult.addEventListener("click", (e) => {
    const btn = e.target.closest("button");
    if (!btn || btn.disabled) return;
    selectRadio(el.ult, btn.dataset.value);
    setUlt(btn.dataset.value);
    call("set_ult", { mode: btn.dataset.value }).catch(() => renderState(state));
  });

  el.reconnect.addEventListener("click", () => call("connect", { address: null }).catch(() => {}));

  // Device picker
  async function openDevices() {
    el.popover.classList.remove("hidden");
    el.deviceList.innerHTML = '<li class="muted">Loading…</li>';
    try {
      const devices = await invoke("list_devices");
      el.deviceList.innerHTML = "";
      if (!devices.length) {
        el.deviceList.innerHTML = '<li class="muted">No paired Sony devices</li>';
      }
      for (const d of devices) {
        const li = document.createElement("li");
        const name = document.createElement("span");
        name.textContent = d.name;
        const sub = document.createElement("small");
        sub.textContent = `${d.address}${d.connected ? " · connected" : ""}`;
        li.append(name, sub);
        li.addEventListener("click", () => {
          el.popover.classList.add("hidden");
          call("connect", { address: d.address }).catch(() => {});
        });
        el.deviceList.append(li);
      }
    } catch (e) {
      el.deviceList.innerHTML = "";
      const li = document.createElement("li");
      li.className = "muted";
      li.textContent = String(e);
      el.deviceList.append(li);
    }
  }
  el.btnDevices.addEventListener("click", (e) => {
    e.stopPropagation();
    if (el.popover.classList.contains("hidden")) openDevices();
    else el.popover.classList.add("hidden");
  });
  el.btnAuto.addEventListener("click", () => {
    el.popover.classList.add("hidden");
    call("connect", { address: null }).catch(() => {});
  });
  document.addEventListener("click", (e) => {
    if (!el.popover.contains(e.target)) el.popover.classList.add("hidden");
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") el.popover.classList.add("hidden");
  });

  // Window controls (frameless window)
  const win = T && T.window ? T.window.getCurrentWindow() : null;
  $("btn-min").addEventListener("click", () => win && win.minimize());
  $("btn-close").addEventListener("click", () => win && win.close());

  // Mouse wheel over the volume slider nudges volume.
  el.volume.addEventListener("wheel", (e) => {
    if (el.volume.disabled) return;
    e.preventDefault();
    el.volume.value = Math.max(0, Math.min(100, +el.volume.value + (e.deltaY < 0 ? 5 : -5)));
    el.volume.dispatchEvent(new Event("input"));
    clearTimeout(el.volume._wheelEnd);
    el.volume._wheelEnd = setTimeout(() => { dragging.volume = false; }, 400);
  }, { passive: false });

  // Gentle 3D tilt of the headphones following the pointer.
  const art = $("art"), phones = art.querySelector(".phones");
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  if (!reduceMotion) {
    document.addEventListener("pointermove", (e) => {
      const r = art.getBoundingClientRect();
      const dx = (e.clientX - (r.left + r.width / 2)) / window.innerWidth;
      const dy = (e.clientY - (r.top + r.height / 2)) / window.innerHeight;
      phones.style.setProperty("--rx", (dx * 14).toFixed(2) + "deg");
      phones.style.setProperty("--ry", (-dy * 10).toFixed(2) + "deg");
    });
    document.addEventListener("pointerleave", () => {
      phones.style.setProperty("--rx", "0deg");
      phones.style.setProperty("--ry", "0deg");
    });
  }

  // Keyboard: 1/2/3 noise modes, U cycles ULT, arrows change volume.
  const clickRadio = (group, value) => {
    const b = group.querySelector(`button[data-value="${value}"]`);
    if (b && !b.disabled) b.click();
  };
  document.addEventListener("keydown", (e) => {
    if (e.target.matches("input") || e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "1") clickRadio(el.anc, "off");
    else if (k === "2") clickRadio(el.anc, "noise_cancelling");
    else if (k === "3") clickRadio(el.anc, "ambient");
    else if (k === "u" && state && state.ult) {
      const order = ["off", "ult1", "ult2"];
      clickRadio(el.ult, order[(order.indexOf(state.ult) + 1) % 3]);
    } else if ((k === "arrowup" || k === "arrowdown") && !el.volume.disabled) {
      e.preventDefault();
      el.volume.value = Math.max(0, Math.min(100, +el.volume.value + (k === "arrowup" ? 5 : -5)));
      el.volume.dispatchEvent(new Event("input"));
      dragging.volume = false;
    }
  });

  // ------------------------------------------------------------ boot

  renderState(null);
  listen("state", (e) => renderState(e.payload));
  listen("connection", (e) => renderConnection(e.payload));
  invoke("get_state")
    .then((snap) => { renderConnection(snap.connection); renderState(snap.state); })
    .catch((e) => toast(String(e)));
})();
