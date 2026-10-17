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
    batteryLevel: $("battery-level"), batterySub: $("battery-sub"),
    volumeRow: $("volume-row"), volume: $("volume"), volumeValue: $("volume-value"),
    ancCard: $("anc-card"), anc: $("anc"), ambient: $("ambient"), ambientLevel: $("ambient-level"),
    ambientValue: $("ambient-value"), focus: $("focus"), focusRow: $("focus-row"),
    ultCard: $("ult-card"), ult: $("ult"),
    model: $("model"), firmware: $("firmware"), backendTag: $("backend-tag"),
    toast: $("toast"), popover: $("devices-popover"), deviceList: $("device-list"),
    btnDevices: $("btn-devices"), btnAuto: $("btn-auto"),
  };

  let state = null;       // last DeviceState
  let connection = null;  // last Connection
  // While the user drags a slider, ignore incoming values for it.
  const dragging = { volume: false, ambient: false };

  // ------------------------------------------------------------ helpers

  function setPct(range) {
    const min = +range.min, max = +range.max;
    range.style.setProperty("--pct", ((+range.value - min) / (max - min)) * 100 + "%");
  }

  function setUnavailable(node, unavailable) {
    node.classList.toggle("unavailable", unavailable);
    node.querySelectorAll("input, button").forEach((n) => (n.disabled = unavailable));
  }

  function selectSegment(group, value) {
    group.querySelectorAll("button").forEach((b) =>
      b.setAttribute("aria-checked", String(b.dataset.value === value)));
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
    el.status.className = "status " + s;
    let text;
    if (s === "connected") text = "Connected";
    else if (s === "connecting") text = "Connecting…";
    else {
      text = "Disconnected";
      if (c.retry_in != null) text += ` · retrying in ${c.retry_in}s`;
    }
    el.statusText.textContent = text;
    el.status.title = c.error || "";
    if (c.device) el.name.textContent = c.device.name;
    else if (s !== "connected" && !state) el.name.textContent = "Sony Connect";
    document.body.classList.toggle("offline", s !== "connected");
    el.backendTag.textContent = c.backend === "mock" ? " · demo mode (mock device)" : "";
  }

  function renderState(s) {
    state = s;
    s = s || {};

    // Battery
    const b = s.battery;
    if (b && typeof b.level === "number") {
      el.batteryCard.classList.remove("unavailable");
      el.batteryLevel.textContent = b.level + "%";
      el.batteryFill.style.width = Math.max(0, Math.min(100, b.level)) + "%";
      el.batteryFill.classList.toggle("low", b.level <= 15);
      el.batteryFill.classList.toggle("mid", b.level > 15 && b.level <= 35);
      el.battery.classList.toggle("charging", !!b.charging);
      el.batterySub.textContent = b.charging ? "Charging" : "Battery";
    } else {
      el.batteryCard.classList.add("unavailable");
      el.batteryLevel.textContent = "—";
      el.batteryFill.style.width = "0";
      el.battery.classList.remove("charging");
      el.batterySub.textContent = "Battery unavailable";
    }

    // Volume
    const hasVol = typeof s.volume === "number";
    setUnavailable(el.volumeRow, !hasVol);
    if (hasVol) {
      if (!dragging.volume) el.volume.value = s.volume;
      el.volumeValue.textContent = el.volume.value + "%";
    } else {
      el.volumeValue.textContent = "Unavailable";
    }
    setPct(el.volume);

    // ANC
    const hasAnc = typeof s.anc === "string";
    setUnavailable(el.ancCard, !hasAnc);
    selectSegment(el.anc, hasAnc ? s.anc : null);
    el.ambient.classList.toggle("hidden", s.anc !== "ambient");
    const hasLevel = typeof s.ambient_level === "number";
    el.ambientLevel.disabled = !hasLevel;
    if (hasLevel && !dragging.ambient) el.ambientLevel.value = s.ambient_level;
    el.ambientValue.textContent = hasLevel ? el.ambientLevel.value : "Unavailable";
    setPct(el.ambientLevel);
    const hasFocus = typeof s.focus_on_voice === "boolean";
    el.focusRow.classList.toggle("unavailable", !hasFocus);
    el.focus.disabled = !hasFocus;
    el.focus.checked = hasFocus && s.focus_on_voice;

    // ULT
    const hasUlt = typeof s.ult === "string";
    setUnavailable(el.ultCard, !hasUlt);
    selectSegment(el.ult, hasUlt ? s.ult : null);

    // Footer
    el.model.textContent = "Model " + (s.model || "unavailable");
    el.firmware.textContent = "Firmware " + (s.firmware || "unavailable");
  }

  // ------------------------------------------------------------ controls

  const sendVolume = debounce((v) => call("set_volume", { volume: v }).catch(() => {}), 120);
  el.volume.addEventListener("input", () => {
    dragging.volume = true;
    el.volumeValue.textContent = el.volume.value + "%";
    setPct(el.volume);
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
    selectSegment(el.anc, btn.dataset.value);
    el.ambient.classList.toggle("hidden", btn.dataset.value !== "ambient");
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
    selectSegment(el.ult, btn.dataset.value);
    call("set_ult", { mode: btn.dataset.value }).catch(() => renderState(state));
  });

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

  // ------------------------------------------------------------ boot

  renderState(null);
  listen("state", (e) => renderState(e.payload));
  listen("connection", (e) => renderConnection(e.payload));
  invoke("get_state")
    .then((snap) => { renderConnection(snap.connection); renderState(snap.state); })
    .catch((e) => toast(String(e)));
})();
