// Copyright (C) 2026 Javad Rajabzadeh
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Content script, three jobs:
//  1. Stamp Alt+clicks so the background honors the "hold Alt to let the
//     browser download normally" convention.
//  2. Collect page links for "Download all links with Hydra".
//  3. The selection pill: highlight one or more links and a floating
//     "Download with Hydra" button appears next to the selection.

window.addEventListener(
  "mousedown",
  (e) => {
    if (e.altKey) {
      try {
        chrome.runtime.sendMessage({ type: "alt-click" });
      } catch {
        // Extension reloaded underneath us; harmless.
      }
    }
  },
  true
);

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg?.type === "collect-links") {
    const urls = Array.from(document.querySelectorAll("a[href]"), (a) => a.href).filter(
      (h) => /^https?:/i.test(h)
    );
    sendResponse({ urls });
  }
  return false;
});

// ------------------------------------------------------- selection pill

const URL_RE = /https?:\/\/[^\s<>"')\]]+/g;

function linksInSelection() {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return [];
  const urls = new Set();
  for (let i = 0; i < sel.rangeCount; i++) {
    const range = sel.getRangeAt(i);
    const root = range.commonAncestorContainer;
    const scope = root.nodeType === Node.ELEMENT_NODE ? root : root.parentElement;
    if (!scope) continue;
    for (const a of scope.querySelectorAll("a[href]")) {
      try {
        if (range.intersectsNode(a) && /^https?:/i.test(a.href)) urls.add(a.href);
      } catch {
        // Detached node mid-mutation; skip it.
      }
    }
  }
  // Bare URLs pasted as text count too.
  for (const m of sel.toString().matchAll(URL_RE)) urls.add(m[0]);
  return [...urls];
}

let pillHost = null;
let pillLabel = null;
let pillUrls = [];

function buildPill() {
  pillHost = document.createElement("div");
  // The page must not style or see into this; a closed shadow root and
  // max z-index keep the pill above and apart.
  const shadow = pillHost.attachShadow({ mode: "closed" });
  pillHost.style.cssText =
    "position:absolute;z-index:2147483647;top:0;left:0;width:0;height:0;overflow:visible;";
  const style = document.createElement("style");
  style.textContent = `
    .pill {
      position: absolute;
      display: flex;
      align-items: center;
      gap: 6px;
      white-space: nowrap;
      font: 12px/1 system-ui, sans-serif;
      color: #1c3a1e;
      background: linear-gradient(#fdfdfd, #e8efe8);
      border: 1px solid #9db89f;
      border-radius: 14px;
      box-shadow: 0 2px 8px rgba(0,0,0,.25);
      padding: 6px 10px;
      cursor: pointer;
      user-select: none;
    }
    .pill:hover { background: linear-gradient(#ffffff, #dcebdd); }
    .icon {
      width: 16px;
      height: 16px;
      flex: 0 0 16px;
      display: block;
      /* The mark is a tall glyph; nudge it onto the text baseline. */
      margin-top: -1px;
    }
    .close {
      font-size: 11px;
      color: #777;
      padding: 2px 4px;
      cursor: pointer;
      border-radius: 8px;
    }
    .close:hover { background: #d6d6d6; color: #333; }
  `;
  const pill = document.createElement("div");
  pill.className = "pill";

  // Brand mark. An <img> with a data: URI is subject to the PAGE's CSP, so
  // a strict img-src would blank it — on failure we swap in an inline SVG
  // arrow, which no CSP can block because nothing is fetched.
  const icon = document.createElement("img");
  icon.className = "icon";
  icon.alt = "";
  icon.src = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACQAAAAkCAYAAADhAJiYAAAAAXNSR0IArs4c6QAAACBjSFJNAAB6JgAAgIQAAPoAAACA6AAAdTAAAOpgAAA6mAAAF3CculE8AAAARGVYSWZNTQAqAAAACAABh2kABAAAAAEAAAAaAAAAAAADoAEAAwAAAAEAAQAAoAIABAAAAAEAAAAkoAMABAAAAAEAAAAkAAAAACoO4/wAAAhvSURBVFgJ5Zd5bFXHFYfPzNz1bX6LV7DBkQlhaQBTFrM5UPa4YUsggRYKSYEqKbQSVRrSqqgoCgmB0AARUCkgQikNNLS0DQgi1oJSCoQEKGFxggDjhWfst/jd++4203GFybOx4WEqpVLvPzP3nDO/+XTmzNy5AP9jD3ponpcWh9xBNR9Jaoh4ZEx97gSSpHBMlqth2rSGB9VvH9Dw2YpalDEFedXnsMvVDylytuB1EayqQBQJsEvRBFWtJqpyxlGE/TbVP6gufTKcDtwDA4kTZw5EHvdvBL+vBCsKMD0JzLIAIQxIEoF4PUD8HhAzvEACfpDzcwFkfErH1vjqvqX3hRLSoW6KIWVTy4CQ33EQPySTYF25AU7S5G5mAhE1nik/liRAPEtEEWvMaOyXrm5F80KTR/eXCH6dB85t0mqrxW057rKXlnVHlGwmbtUvYgbW9ZvgNCTOU8t4kUXjU+zaWxNBEFaDy8Uz1bhsrhy1sKA0WVm9MXb4n1GMyAsFn51cdJduC0O6QFgA/BrCOCTz5TDCEaCOvdGurx2KMc5EgriR2M5O6/Mz+xi1F/MsaqxxCV3uma78gnU0HPHon5xBAmMrOl38fFYLhmav6QEVDwsBgycwrxHKEDiG8YGz/48vSL27j8bZmUuZRBLc/xIvpnPm1rVvUFMrtgxjjK3pixjGx4jPR6zKOtD+cQ5QJNqnGUGLlzRrSBEBYQIEA7MpYEkBhwsJgcCzNNqQZECfsc7t/7RJ23j/3Uu8f8kA+BimTn3HXzykDHmUZ/WqmjA9D8ub4lpr09tljw3xktyssyTD01nOzwMzqpnU1geoj3Ra7ETieYn1K59oTbw9tvSW7OKxODB6jpo2UMsEwe+VxEDwTcboVuzzNGbjv/aQVpU2bBBd2QXflQaX5lv9i8Nw4oSJO3aVECKTkGWDlB0E7PV0QQSdB2TsNHv0qIFTp2irWilGz492ZssDfzhKHD833zq86UqK6073riUrPP1JYfzP+zfoX1aMEXNCIGaHLqKAZ3n4iy+2CSe+Os639ONy51wQiwoAibIJiM2oe/mnH95RbK0z+6DiD+KfY1GaxxjqwGgS8JDMzXhozoLarKx46pBmGQps354hhSO7EyfOD3V0nbGEZtnlFTnm5YqJQmVtvZmIrcGC9H0nlhAFnwfk3ExCPK6xrimTjmm7dl1LFb7Tn7qduMWqzUyPLHAiN72Y0QSyiSQGPX1wHipIBP274NAh1hTfrIakG1Wz7IqavlZN2OZAc7Wjp/slL13daV6rASuWfF3gxzHTGuYyy2b62cvgRKIg+70Zqs+3rdP27T2aRFNbdy5dgNyu6XaiDowbl1bqFw/3sQ1jJQ3H+WmufC9rzpzBqfHNgPRL14clyq+DoMjnrI/+8B5UnDrr0MoZAHQfllUJ+/xbHcVzgOrJRVQ3IH7kFLBYHES/r6MU8G0pPHjQnyru/tXe0cTtfY0B4ccUXmUdWv4z49Cq8viOM8tonhhBfh/iJVCSOqYZkHG1en/yWjU/YEgn76z5E/8TWF5u2IbN1968DqpcKGd694qdc//GDG0T3/JQf+A4MMcG7Mvoi0T4RZO499d/f0qUlG38FHdTI3nUQCm+1SOfF0s6umk0YduGc6xpTGPbvKjzB6lCx8BekORhcnaGxQHWJMLhFbB3ZxWUjB8iZoZ2C4EMH5JwhVkdngGV9b8HjPNDz08AV79vATOMOr3a6WddCP4YSWwhQ1SwblZets9XfEffs7BCWnGkh5rpXiL0DEwT8gPA4g2/vdm1YH4qULOihliFTd05hwGjUVSzcgWXOkjukPOcOKg0E3d95Ci9XvURuOReOBh8TMjxx5x/fXmKEWEYOBYoXToB0i3V0bXTLBKcA4KY7TRE9yQborPlmSUh5Zn5S6UM+W2hKNQXMlSwYw17ENbnaWvXmqlAzTPU5OkyPJ9kKKv5R3KylBUEuVshYL8ngVX5L/zusyNxvSZIfMrI5P6Tu8GgW4SsDAjNGAeARbDrI0udyMBaS3JqnZ5eQfHKsxh1RmAsEv5dA5vYFpJgXSSmvwpjeyeapmxqWwe67SXFY5/mN69FHGSQq3sReIq78VpxMyLLuwzirKpauCxGgpmnsccNvnGDeC0x3YpHptNvT9dFlSxBbmkwQQRQ3AHH1G1bIXv5Ir7VMKHv4SaAlu09gW4HC0KvkaMcQZwr5wbLAiNKZHevrvxL4sR4sS66+pMVAZKXtVzq1UWjNpuj9JvVB3nxK8gtItAYsFtaHb+K7HBktik2v9/xlgAt39MB+npMz2EDSCD4SnDEgMne3t3AThqO3RCbWbPuw9GsQ+B8aPziALikV6nkAL2lmSxireeF/k70jVFffS1y796DAd3WwgPHvegv7b9S7lyg2InEreiFk6Vqh9ndpazQDqZgxKJalV2rz46+NXLfvae/29t8l93tb9XCbpSfsJXQTeT1lDGRuLHojknu3mX8Lv0o1fS4dSMyKfb2mIOtDr6PsV1AjZr0yoVPhaLuxSDL3cAQeohKYRcQBdGJxpfFlo/ecp9523Q3O6nbjGrDYTck3rXr6vk1mob4lnY5mlZv1OvvtRGelvmhgJIUTvMlCrMkv6wixHcenNXXPFmR1sxtBD0UEPzp/Vs0odfRRiCM+HcIVbYxT9rmdu0yGD5cEYTQEl4zj/LUjCauoM/XZwZQQq5RVT5uWMZB7eWS9dx3556TLlH7MnToUBJM82OwWBnY1Md0E6ht8x9EtRMzzGLRsfe2B6YRun1AfKB95K8HWEN8GmjJJDItbiHgJLRrTlR/Krp4WNoHYSNE6tPubd8owirKLyFfXg2WvROEvMcjtsMmx5eN+Cx1gm+kLw35wWLP3C1PfyOT/99N+m9hmXZtaNkxfgAAAABJRU5ErkJggg==";
  icon.addEventListener("error", () => {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("class", "icon");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "M12 3v12m0 0l-5-5m5 5l5-5M4 20h16");
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", "#1c7a6e");
    path.setAttribute("stroke-width", "2.4");
    path.setAttribute("stroke-linecap", "round");
    path.setAttribute("stroke-linejoin", "round");
    svg.append(path);
    icon.replaceWith(svg);
  });

  pillLabel = document.createElement("span");
  const close = document.createElement("span");
  close.className = "close";
  close.textContent = "✕";
  pill.append(icon, pillLabel, close);
  shadow.append(style, pill);

  pill.addEventListener("mousedown", (e) => {
    // Keep the selection alive: no default mousedown, no bubbling into the
    // page's own selection handling.
    e.preventDefault();
    e.stopPropagation();
  });
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    hidePill();
  });
  pill.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!pillUrls.length) return hidePill();
    pillLabel.textContent = "Sending…";
    try {
      chrome.runtime.sendMessage(
        { type: "selection-links", urls: pillUrls, referer: location.href },
        (r) => {
          pillLabel.textContent = r && r.ok ? "Sent to Hydra ✓" : "Hydra not reachable";
          setTimeout(hidePill, 1200);
        }
      );
    } catch {
      hidePill();
    }
  });
  document.documentElement.append(pillHost);
  return pill;
}

let pillEl = null;

function hidePill() {
  if (pillEl) pillEl.style.display = "none";
}

function showPill() {
  const urls = linksInSelection();
  if (!urls.length) return hidePill();
  const sel = window.getSelection();
  const rect = sel.getRangeAt(sel.rangeCount - 1).getBoundingClientRect();
  if (!rect || (rect.width === 0 && rect.height === 0)) return hidePill();

  if (!pillEl) pillEl = buildPill();
  pillUrls = urls;
  pillLabel.textContent =
    urls.length === 1 ? "Download with Hydra" : `Download ${urls.length} links with Hydra`;
  pillEl.style.display = "flex";
  // Below the selection's end, clamped to the viewport; above when there is
  // no room underneath.
  const margin = 6;
  let top = window.scrollY + rect.bottom + margin;
  if (rect.bottom + 34 > window.innerHeight) top = window.scrollY + rect.top - 34;
  let left = window.scrollX + Math.min(Math.max(rect.left, 4), window.innerWidth - 240);
  pillEl.style.top = `${top}px`;
  pillEl.style.left = `${left}px`;
}

document.addEventListener("mouseup", (e) => {
  if (pillHost && e.composedPath().includes(pillHost)) return;
  // Let the browser finalize the selection first.
  setTimeout(showPill, 10);
});

document.addEventListener("selectionchange", () => {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed) hidePill();
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") hidePill();
});

window.addEventListener("scroll", hidePill, { passive: true });
