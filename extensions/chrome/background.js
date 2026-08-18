// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Hydra browser integration, service worker.
//
// Architecture:
//  - Primary transport: a persistent WebSocket to the app's fixed loopback
//    port (hydra: 6799/16799), reconnecting with backoff forever. The open
//    socket doubles as the "app is running" indicator (toolbar shows a gray
//    X when it is down).
//  - Fallback transport: native messaging through `hydra-host`, which can
//    LAUNCH the app when it is not running, after which the WebSocket
//    takes over again.
//  - Capture: pause the browser download the moment it is created (pausing
//    is reversible where cancelling is not), decide when the filename is
//    known, then either hand it to Hydra and erase, or resume the browser
//    download untouched.

// One source of truth for Chrome/Edge/Brave AND Safari. Safari exposes the
// promise-based `browser` namespace (and `chrome` only since 16.4), so bind
// whichever exists and feature-detect the rest: Safari has no `downloads`
// API at all, which is why auto-capture is Chromium-only. The WebSocket
// transport below is identical everywhere.
globalThis.chrome ??= globalThis.browser;

const HOST = "com.hydra.host";
const WS_PORTS = [6799, 16799];

// Mirrors hydra-gui's Options > File Types default; overwritten by the GUI's
// list on every round-trip.
const DEFAULT_TYPES =
  "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 DMG EXE GZ GZIP IMG ISO LZH " +
  "M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PKG PPS PPT " +
  "QT RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP";
const DEFAULT_SKIP = "*.update.microsoft.com download.windowsupdate.com";

// Content-type → canonical extension. Used when the filename itself is
// inconclusive.
const MIME_EXT = {
  "application/zip": "ZIP", "application/x-zip": "ZIP", "application/x-zip-compressed": "ZIP",
  "application/gzip": "GZ", "application/x-gzip": "GZ", "application/x-gzip-compressed": "GZ",
  "application/x-7z-compressed": "7Z", "application/x-compress-7z": "7Z",
  "application/x-rar": "RAR", "application/x-rar-compressed": "RAR",
  "application/x-tar": "TAR", "application/x-gtar": "TAR",
  "application/x-bzip2": "BZ2", "application/x-compress": "Z",
  "application/pdf": "PDF", "application/x-msi": "MSI",
  "application/x-dosexec": "EXE", "application/x-msdos-program": "EXE",
  "application/x-apple-diskimage": "DMG",
  "application/vnd.android.package-archive": "APK",
  "application/epub+zip": "EPUB",
  "video/mp4": "MP4", "video/x-mp4": "MP4", "video/mpg4": "MP4",
  "video/webm": "WEBM", "video/x-matroska": "MKV", "video/quicktime": "MOV",
  "video/x-msvideo": "AVI", "video/msvideo": "AVI", "video/avi": "AVI",
  "video/x-flv": "FLV", "video/x-flash-video": "FLV",
  "video/mpeg": "MPG", "video/x-ms-wmv": "WMV", "video/x-ms-asf": "ASF",
  "video/3gpp": "3GP", "video/3gpp2": "3GP",
  "audio/mpeg": "MP3", "audio/mp3": "MP3", "audio/x-mpeg": "MP3",
  "audio/mp4": "M4A", "audio/mp4a-latm": "M4A", "audio/aac": "AAC",
  "audio/wav": "WAV", "audio/x-wav": "WAV", "audio/x-ms-wma": "WMA",
  "audio/ogg": "OGG", "audio/webm": "WEBM", "audio/flac": "FLAC",
};

// Direct media worth listing in the popup sniffer (no segmented HLS/DASH —
// a plain downloader cannot merge streams).
const MEDIA_MIME =
  /^(video\/(mp4|webm|x-matroska|quicktime|x-msvideo|x-flv|mpeg)|audio\/(mpeg|mp4|aac|ogg|wav|flac|webm))/i;
const MEDIA_MIN_BYTES = 256 * 1024;
const MEDIA_PER_TAB = 25;

// ---------------------------------------------------------------- settings

async function getState() {
  return chrome.storage.local.get({
    enabled: true, // extension-side master switch (popup toggle)
    guiCapture: true, // "Google Chrome" checkbox in hydra's Options
    autoTypes: DEFAULT_TYPES,
    skipSites: DEFAULT_SKIP,
    hydraSeen: false, // ever completed a round-trip
  });
}

function absorbReply(reply) {
  if (!reply || typeof reply !== "object" || reply.unreachable) return;
  const patch = { hydraSeen: true };
  if (typeof reply.capture === "boolean") patch.guiCapture = reply.capture;
  if (typeof reply.auto_types === "string" && reply.auto_types) patch.autoTypes = reply.auto_types;
  if (typeof reply.dont_start_sites === "string") patch.skipSites = reply.dont_start_sites;
  chrome.storage.local.set(patch);
}

// --------------------------------------------------------------- transport

let ws = null;
let wsReady = false;
let wsPortIdx = 0;
let reconnectTimer = null;
let heartbeatTimer = null;
let reqSeq = 1;
const pendingReqs = new Map(); // id -> {resolve, timer}

function setConnected(on) {
  // Gray X on the toolbar icon while the app is down.
  chrome.action.setBadgeBackgroundColor({ color: "#606060" }).catch(() => {});
  chrome.action.setBadgeText({ text: on ? "" : "X" }).catch(() => {});
  chrome.action
    .setTitle({ title: on ? "Hydra Download Manager" : "Hydra is not running" })
    .catch(() => {});
}

function wsTeardown() {
  wsReady = false;
  if (heartbeatTimer) {
    clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  for (const [, p] of pendingReqs) {
    clearTimeout(p.timer);
    p.resolve(null);
  }
  pendingReqs.clear();
  if (ws) {
    try {
      ws.close();
    } catch {}
    ws = null;
  }
}

function wsScheduleReconnect(delayMs = 4000) {
  if (reconnectTimer) return;
  wsPortIdx++; // next attempt tries the fallback port
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    wsConnect();
  }, delayMs);
}

function wsConnect() {
  if (ws) return;
  try {
    ws = new WebSocket(`ws://127.0.0.1:${WS_PORTS[wsPortIdx % WS_PORTS.length]}/`);
  } catch {
    ws = null;
    wsScheduleReconnect();
    return;
  }
  ws.onopen = () => {
    wsReady = true;
    setConnected(true);
    // Heartbeat: keeps the service worker alive (Chrome extends SW life on
    // WS traffic) and notices a dead app quickly.
    heartbeatTimer = setInterval(() => wsRequest({ type: "ping" }, 5000), 20000);
    wsRequest({ type: "config" }, 5000); // sync capture settings on connect
  };
  ws.onmessage = (ev) => {
    let reply;
    try {
      reply = JSON.parse(ev.data);
    } catch {
      return;
    }
    absorbReply(reply);
    const p = pendingReqs.get(reply.id);
    if (p) {
      clearTimeout(p.timer);
      pendingReqs.delete(reply.id);
      p.resolve(reply);
    }
  };
  ws.onclose = ws.onerror = () => {
    wsTeardown();
    setConnected(false);
    wsScheduleReconnect();
  };
}

/// Returns the reply, or null when the socket is down / timed out — the
/// caller then falls back to the native host.
function wsRequest(msg, timeoutMs = 8000) {
  return new Promise((resolve) => {
    if (!ws || !wsReady) return resolve(null);
    const id = reqSeq++;
    const timer = setTimeout(() => {
      pendingReqs.delete(id);
      resolve(null);
    }, timeoutMs);
    pendingReqs.set(id, { resolve, timer });
    try {
      ws.send(JSON.stringify({ ...msg, id }));
    } catch {
      clearTimeout(timer);
      pendingReqs.delete(id);
      resolve(null);
    }
  });
}

/// Native messaging, both dialects: Chrome takes a callback and reports
/// failures via lastError; Safari ignores the host name (the message goes
/// to the wrapper app's SFSafariWebExtensionHandler) and returns a promise.
function native(msg) {
  return new Promise((resolve) => {
    const done = (reply) => {
      absorbReply(reply);
      resolve(reply || { ok: false, error: "no reply", unreachable: true });
    };
    const failed = (e) => resolve({ ok: false, error: String(e), unreachable: true });
    try {
      const ret = chrome.runtime.sendNativeMessage(HOST, msg, (reply) => {
        if (chrome.runtime.lastError) failed(chrome.runtime.lastError.message);
        else done(reply);
      });
      if (ret && typeof ret.then === "function") ret.then(done, failed);
    } catch (e) {
      failed(e);
    }
  });
}

/// One request, best transport: WebSocket when live; otherwise the native
/// host (which launches the app), after which the WebSocket reattaches.
async function request(msg) {
  const viaWs = await wsRequest(msg);
  if (viaWs) return viaWs;
  const viaHost = await native(msg);
  if (viaHost && viaHost.ok) {
    setConnected(true);
    if (!reconnectTimer) wsScheduleReconnect(700); // app is up: reattach fast
  }
  return viaHost;
}

// ----------------------------------------------------------------- filters

function extOf(nameOrPath) {
  const base = String(nameOrPath || "").split(/[/\\]/).pop().split(/[?#]/)[0];
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toUpperCase() : "";
}

function typeMatches(autoTypes, filename, url, mime) {
  const set = new Set(autoTypes.toUpperCase().split(/[\s,]+/).filter(Boolean));
  const fromName = extOf(filename);
  if (fromName && set.has(fromName)) return true;
  const mapped = MIME_EXT[(mime || "").split(";")[0].trim().toLowerCase()];
  if (mapped && set.has(mapped)) return true;
  try {
    const fromUrl = extOf(new URL(url).pathname);
    return !!fromUrl && set.has(fromUrl);
  } catch {
    return false;
  }
}

// Site patterns support leading-* wildcard ("*.update.microsoft.com").
function siteSkipped(skipSites, url) {
  let host;
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return false;
  }
  return skipSites
    .toLowerCase()
    .split(/[\s,]+/)
    .filter(Boolean)
    .some((pat) => {
      if (pat.startsWith("*.")) {
        const dom = pat.slice(2);
        return host === dom || host.endsWith("." + dom);
      }
      return host === pat || host.endsWith("." + pat);
    });
}

// `storage.session` is MV3/Safari 16.4+; fall back to local where absent so
// the media list and the Alt stamp still work (they are short-lived keys).
const sessionStore = () => chrome.storage.session ?? chrome.storage.local;

async function altBypassed() {
  const { altTs = 0 } = await sessionStore().get("altTs");
  return Date.now() - altTs < 3000;
}

// ----------------------------------------------------------------- cookies

async function cookieHeader(url) {
  try {
    const cookies = await chrome.cookies.getAll({ url });
    if (!cookies.length) return null;
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return null;
  }
}

// ----------------------------------------------------------------- capture

async function sendToHydra(url, extras = {}) {
  return request({
    type: "download",
    url,
    cookies: await cookieHeader(url),
    user_agent: navigator.userAgent,
    ...extras,
  });
}

function captureEligible(item, state) {
  const url = item.finalUrl || item.url;
  return (
    state.enabled &&
    state.guiCapture &&
    /^(https?|ftp):/i.test(url) &&
    item.byExtensionId !== chrome.runtime.id
  );
}

// Freeze the download the moment it exists (downloads.pause in onCreated).
// Pausing is reversible where cancelling is not: small files
// cannot slip through by finishing early, and signed one-shot URLs keep
// working because we never have to re-request them.
async function parkDownload(item) {
  const state = await getState();
  if (!captureEligible(item, state)) return;
  try {
    await chrome.downloads.pause(item.id);
  } catch {
    // Finished already or not pausable; the decision step handles it.
  }
}

async function decideCapture(item) {
  const state = await getState();
  const url = item.finalUrl || item.url;

  const giveBack = async () => {
    try {
      await chrome.downloads.resume(item.id);
    } catch {}
  };

  if (!captureEligible(item, state)) return giveBack();
  if (siteSkipped(state.skipSites, url)) return giveBack();
  if (await altBypassed()) return giveBack();
  if (!typeMatches(state.autoTypes, item.filename, url, item.mime)) return giveBack();

  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize > 0 ? item.fileSize : null;
  const reply = await sendToHydra(url, {
    filename: item.filename ? item.filename.split(/[/\\]/).pop() : null,
    referer: item.referrer || null,
    size,
    mime: item.mime || null,
  });

  if (reply && reply.ok) {
    // Hydra owns it now; drop the browser's paused copy.
    try {
      await chrome.downloads.cancel(item.id);
    } catch {}
    try {
      await chrome.downloads.erase({ id: item.id });
    } catch {}
  } else {
    // Hydra unreachable: the browser download continues untouched.
    await giveBack();
  }
}

// Looked up by name rather than written as a property access: the event is
// Chromium-only, and a literal `chrome.downloads.onDeterminingFilename`
// makes Firefox's add-on linter report an unsupported API even though this
// branch never runs there.
const onDeterminingFilename = chrome.downloads?.["onDeterminingFilename"];

if (onDeterminingFilename) {
  // Chromium: park on creation, decide once onDeterminingFilename resolves
  // the real name (Content-Disposition applied).
  chrome.downloads.onCreated.addListener((item) => parkDownload(item));
  onDeterminingFilename.addListener((item, suggest) => {
    suggest();
    decideCapture(item);
  });
} else if (chrome.downloads?.onCreated) {
  // Firefox: no onDeterminingFilename, but `filename` is already resolved on
  // the create event. Park and decide in ONE sequential listener — two
  // listeners on the same event run concurrently, and the decision would
  // race the pause it depends on.
  chrome.downloads.onCreated.addListener(async (item) => {
    await parkDownload(item);
    await decideCapture(item);
  });
}
// Safari has no downloads API at all; there the extension is menus +
// selection pill + sniffing only.

// ------------------------------------------------------------ context menus

// Promise form (not the callback): it is the only one both Chrome MV3 and
// Safari's promise-based namespace implement.
async function installMenus() {
  const menus = chrome.contextMenus ?? chrome.menus;
  if (!menus) return;
  try {
    await menus.removeAll();
  } catch {}
  for (const [id, title, contexts] of [
    ["hydra-link", "Download with Hydra", ["link"]],
    ["hydra-media", "Download with Hydra", ["image", "video", "audio"]],
    ["hydra-all-links", "Download all links with Hydra", ["page"]],
  ]) {
    try {
      menus.create({ id, title, contexts });
    } catch {}
  }
}

chrome.runtime.onInstalled.addListener((details) => {
  installMenus();
  wsConnect();
  if (details?.reason === "install" && chrome.tabs?.create) {
    chrome.tabs.create({ url: chrome.runtime.getURL("welcome.html") });
  }
});
chrome.runtime.onStartup.addListener(() => {
  installMenus();
  wsConnect();
});
// Any service-worker wake-up re-establishes the socket.
wsConnect();

(chrome.contextMenus ?? chrome.menus)?.onClicked.addListener(async (info, tab) => {
  if (info.menuItemId === "hydra-link" || info.menuItemId === "hydra-media") {
    const url = info.menuItemId === "hydra-link" ? info.linkUrl : info.srcUrl;
    if (url) await sendToHydra(url, { referer: tab?.url || null, tab_url: tab?.url || null });
  } else if (info.menuItemId === "hydra-all-links" && tab?.id != null) {
    try {
      const resp = await chrome.tabs.sendMessage(tab.id, { type: "collect-links" });
      const urls = [...new Set((resp?.urls || []).filter((u) => /^https?:/i.test(u)))];
      if (urls.length) await request({ type: "links", urls });
    } catch {
      // No content script on this page (chrome://, PDF viewer, ...).
    }
  }
});

// ------------------------------------------------------------ media sniffer

async function tabMedia(tabId) {
  const key = `media_${tabId}`;
  const got = await sessionStore().get(key);
  return got[key] || [];
}

async function setMediaBadge(tabId, count) {
  try {
    await chrome.action.setBadgeBackgroundColor({ color: "#2c6e31", tabId });
    await chrome.action.setBadgeText({ tabId, text: count ? String(count) : "" });
  } catch {
    // Tab may already be gone.
  }
}

chrome.webRequest?.onResponseStarted.addListener(
  async (details) => {
    if (details.tabId < 0) return;
    const headers = Object.fromEntries(
      (details.responseHeaders || []).map((h) => [h.name.toLowerCase(), h.value])
    );
    const mime = (headers["content-type"] || "").split(";")[0].trim();
    if (!MEDIA_MIME.test(mime)) return;
    const size = parseInt(headers["content-length"] || "0", 10) || 0;
    // Range replies of streaming players still reveal the full size here.
    const total = /\/(\d+)$/.exec(headers["content-range"] || "");
    const fullSize = total ? parseInt(total[1], 10) : size;
    if (fullSize && fullSize < MEDIA_MIN_BYTES) return;

    const url = details.url.split("#")[0];
    const key = `media_${details.tabId}`;
    const list = await tabMedia(details.tabId);
    // Same resource re-requested with shifting Range offsets: keep one entry.
    const bare = url.split("?")[0];
    if (!list.some((m) => m.url.split("?")[0] === bare)) {
      list.push({ url, mime, size: fullSize || null });
      if (list.length > MEDIA_PER_TAB) list.shift();
      await sessionStore().set({ [key]: list });
      setMediaBadge(details.tabId, list.length);
    }
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "other"] },
  ["responseHeaders"]
);

async function clearTab(tabId) {
  await sessionStore().remove(`media_${tabId}`);
  setMediaBadge(tabId, 0);
}
chrome.tabs.onRemoved.addListener((tabId) => clearTab(tabId));
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.url) clearTab(tabId); // navigated away: stale list
});

// ------------------------------------------------------------ popup/content

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {
    switch (msg?.type) {
      case "alt-click":
        await sessionStore().set({ altTs: Date.now() });
        sendResponse({ ok: true });
        break;
      case "get-state": {
        const state = await getState();
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        const media = tab?.id != null ? await tabMedia(tab.id) : [];
        sendResponse({
          state,
          media,
          connected: wsReady,
          tabId: tab?.id ?? null,
          tabUrl: tab?.url ?? null,
        });
        break;
      }
      case "set-enabled":
        await chrome.storage.local.set({ enabled: !!msg.enabled });
        sendResponse({ ok: true });
        break;
      case "ping":
        // Probe only: never boots the app, so the popup can report status.
        sendResponse((await wsRequest({ type: "ping" }, 2500)) || { ok: false, unreachable: true });
        break;
      case "open-hydra":
        sendResponse(await request({ type: "open" }));
        break;
      case "download-url":
        sendResponse(await sendToHydra(msg.url, { referer: msg.referer || null }));
        break;
      case "selection-links": {
        // The floating pill over highlighted links: one link behaves like a
        // click on it, several open the batch box.
        const urls = [...new Set((msg.urls || []).filter((u) => /^https?:/i.test(u)))];
        if (!urls.length) return sendResponse({ ok: false, error: "no links" });
        if (urls.length === 1) {
          sendResponse(await sendToHydra(urls[0], { referer: msg.referer || null }));
        } else {
          sendResponse(await request({ type: "links", urls }));
        }
        break;
      }
      case "download-all-links": {
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        if (tab?.id == null) return sendResponse({ ok: false, error: "no tab" });
        try {
          const resp = await chrome.tabs.sendMessage(tab.id, { type: "collect-links" });
          const urls = [...new Set((resp?.urls || []).filter((u) => /^https?:/i.test(u)))];
          if (!urls.length) return sendResponse({ ok: false, error: "no links" });
          sendResponse(await request({ type: "links", urls }));
        } catch {
          sendResponse({ ok: false, error: "page not accessible" });
        }
        break;
      }
      default:
        sendResponse({ ok: false, error: "unknown message" });
    }
  })();
  return true; // async sendResponse
});
