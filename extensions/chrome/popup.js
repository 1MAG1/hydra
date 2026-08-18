// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later

const $ = (id) => document.getElementById(id);

function fmtSize(n) {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(n >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function nameOf(url) {
  try {
    const p = new URL(url).pathname.split("/").filter(Boolean).pop();
    return decodeURIComponent(p || url);
  } catch {
    return url;
  }
}

async function refresh() {
  const { state, media, tabUrl, connected } = await chrome.runtime.sendMessage({
    type: "get-state",
  });
  $("enabled").checked = state.enabled;

  // Liveness: the live WebSocket is the truth; probe only if it looks down
  // (never launches the app).
  const ping = connected ? { ok: true } : await chrome.runtime.sendMessage({ type: "ping" });
  const st = $("status");
  if (ping && ping.ok) {
    st.className = "status on";
    st.title = "Hydra is running";
  } else {
    st.className = "status off";
    st.title = state.hydraSeen
      ? "Hydra is not running (it starts automatically on capture)"
      : "Hydra native host not reachable — run the install script";
  }

  const hint = $("hint");
  if (!state.guiCapture) {
    hint.textContent = "Capture is off in Hydra's Options (Google Chrome unchecked).";
  } else if (!(ping && ping.ok) && !state.hydraSeen) {
    hint.textContent = "Install the native host: scripts/install-native-host.sh";
  } else {
    hint.textContent = "Alt+click a link to bypass capture once.";
  }

  const list = $("media-list");
  list.textContent = "";
  $("media-section").hidden = !media.length;
  for (const m of media) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = nameOf(m.url);
    name.title = m.url;
    const size = document.createElement("span");
    size.className = "size";
    size.textContent = fmtSize(m.size);
    const btn = document.createElement("button");
    btn.textContent = "Download";
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      btn.textContent = "Sent";
      await chrome.runtime.sendMessage({ type: "download-url", url: m.url, referer: tabUrl });
    });
    li.append(name, size, btn);
    list.append(li);
  }
}

$("enabled").addEventListener("change", async (e) => {
  await chrome.runtime.sendMessage({ type: "set-enabled", enabled: e.target.checked });
});

$("open-hydra").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "open-hydra" });
  window.close();
});

$("all-links").addEventListener("click", async () => {
  const r = await chrome.runtime.sendMessage({ type: "download-all-links" });
  $("hint").textContent = r && r.ok ? "Links sent to Hydra." : `Could not send links${r?.error ? `: ${r.error}` : ""}.`;
});

refresh();
