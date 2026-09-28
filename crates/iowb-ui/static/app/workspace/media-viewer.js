"use strict";

// Browser media elements cannot attach an Authorization header themselves.
// The protected raw-file route accepts the same short-lived browser token as a
// query parameter, which keeps range requests working for <video> while the
// rest of the application continues to use bearer headers.
const MEDIA_VIEWER_IMAGE_EXTENSIONS = new Set([
  "avif",
  "bmp",
  "gif",
  "heic",
  "heif",
  "ico",
  "jpe",
  "jpeg",
  "jpg",
  "png",
  "svg",
  "webp",
]);

const MEDIA_VIEWER_VIDEO_EXTENSIONS = new Set([
  "3gp",
  "3gpp",
  "avi",
  "m4v",
  "mkv",
  "mov",
  "mp4",
  "mpeg",
  "mpg",
  "ogv",
  "webm",
]);

const MEDIA_VIEWER_MAX_ZOOM = 4;
const MEDIA_VIEWER_MIN_ZOOM = 0.5;
const MEDIA_VIEWER_ZOOM_STEP = 0.25;
const MEDIA_VIEWER_SWIPE_DISTANCE = 64;
const MEDIA_VIEWER_SWIPE_VERTICAL_DRIFT = 90;

let mediaViewerReturnFocus = null;
let mediaViewerBound = false;

function mediaViewerExtension(path) {
  return String(path || "")
    .split(/[?#]/, 1)[0]
    .split(".")
    .pop()
    .toLowerCase();
}

function mediaViewerKindForPath(path) {
  const extension = mediaViewerExtension(path);
  if (MEDIA_VIEWER_IMAGE_EXTENSIONS.has(extension)) return "image";
  if (MEDIA_VIEWER_VIDEO_EXTENSIONS.has(extension)) return "video";
  return null;
}

function mediaViewerActionLabel(path) {
  const kind = mediaViewerKindForPath(path);
  if (kind === "image") return "View image";
  if (kind === "video") return "Play video";
  return "Open file";
}

function mediaViewerFileName(path) {
  return String(path || "").split(/[\\/]/).filter(Boolean).pop() || "Media file";
}

function mediaViewerRawUrl(filePath, project = activeProjectKey()) {
  const params = new URLSearchParams({ filePath });
  // A media element cannot send Authorization: Bearer. The server accepts a
  // token query only for protected streaming requests and never stores it in
  // application state beyond the existing session token.
  if (state.token) params.set("token", state.token);
  return `/api/projects/${encodeURIComponent(project)}/files/raw?${params.toString()}`;
}

function mediaViewerItems() {
  return flattenFileEntries(state.fileEntries)
    .filter(({ entry }) => entry.type !== "directory")
    .map(({ entry }) => ({
      path: entry.path,
      name: entry.name,
      kind: mediaViewerKindForPath(entry.path),
      size: entry.size || 0,
    }))
    .filter((item) => item.kind)
    .filter((item, index, items) => items.findIndex((candidate) => candidate.path === item.path) === index);
}

function mediaViewerFocusableElements(dialog) {
  if (!dialog) return [];
  return [...dialog.querySelectorAll(
    'button:not([disabled]), a[href], video, [tabindex]:not([tabindex="-1"])',
  )].filter((element) => !element.hidden && !element.closest(".hidden"));
}

function trapMediaViewerFocus(event, dialog) {
  if (event.key !== "Tab") return;
  const focusable = mediaViewerFocusableElements(dialog);
  if (!focusable.length) return;
  const first = focusable[0];
  const last = focusable.at(-1);
  if (!dialog.contains(document.activeElement)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function mediaViewerZoomLabel(zoom) {
  return `${Math.round(zoom * 100)}%`;
}

function mediaViewerSetStatus(viewer, message, stateName = "loading") {
  const status = viewer?.querySelector("[data-media-viewer-status]");
  if (!status) return;
  status.textContent = message;
  status.dataset.state = stateName;
  status.classList.toggle("hidden", !message);
}

function mediaViewerSetError(viewer, message) {
  const error = viewer?.querySelector("[data-media-viewer-error]");
  if (!error) return;
  error.textContent = message;
  error.classList.toggle("hidden", !message);
}

function mediaViewerApplyImageTransform(viewer) {
  if (!viewer) return;
  const image = viewer.querySelector("[data-media-viewer-image]");
  const zoom = viewer.dataset.zoom ? Number(viewer.dataset.zoom) : 1;
  const offsetX = Number(viewer.dataset.offsetX || 0);
  const offsetY = Number(viewer.dataset.offsetY || 0);
  const label = viewer.querySelector("[data-media-viewer-zoom-label]");
  const zoomOut = viewer.querySelector("[data-media-viewer-zoom-out]");
  const zoomIn = viewer.querySelector("[data-media-viewer-zoom-in]");
  const reset = viewer.querySelector("[data-media-viewer-zoom-reset]");
  if (image) {
    image.style.transform = `translate3d(${offsetX}px, ${offsetY}px, 0) scale(${zoom})`;
    image.classList.toggle("is-zoomed", zoom !== 1 || offsetX !== 0 || offsetY !== 0);
  }
  if (label) label.textContent = mediaViewerZoomLabel(zoom);
  if (zoomOut) zoomOut.disabled = zoom <= MEDIA_VIEWER_MIN_ZOOM;
  if (zoomIn) zoomIn.disabled = zoom >= MEDIA_VIEWER_MAX_ZOOM;
  if (reset) reset.disabled = zoom === 1 && offsetX === 0 && offsetY === 0;
}

function mediaViewerResetImageTransform(viewer) {
  if (!viewer) return;
  viewer.dataset.zoom = "1";
  viewer.dataset.offsetX = "0";
  viewer.dataset.offsetY = "0";
  mediaViewerApplyImageTransform(viewer);
}

function mediaViewerChangeZoom(viewer, amount) {
  if (!viewer || viewer.dataset.kind !== "image") return;
  const current = Number(viewer.dataset.zoom || 1);
  const next = Math.min(
    MEDIA_VIEWER_MAX_ZOOM,
    Math.max(MEDIA_VIEWER_MIN_ZOOM, Math.round((current + amount) * 100) / 100),
  );
  viewer.dataset.zoom = String(next);
  if (next === 1) {
    viewer.dataset.offsetX = "0";
    viewer.dataset.offsetY = "0";
  }
  mediaViewerApplyImageTransform(viewer);
}

function mediaViewerSetItem(viewer, item) {
  if (!viewer || !item) return;
  const image = viewer.querySelector("[data-media-viewer-image]");
  const video = viewer.querySelector("[data-media-viewer-video]");
  const title = viewer.querySelector("[data-media-viewer-title]");
  const kind = viewer.querySelector("[data-media-viewer-kind]");
  const details = viewer.querySelector("[data-media-viewer-details]");
  const position = viewer.querySelector("[data-media-viewer-position]");
  const items = mediaViewerItems();
  const index = items.findIndex((candidate) => candidate.path === item.path);
  const nextItem = items[index >= 0 ? index : 0] || item;
  const rawUrl = mediaViewerRawUrl(nextItem.path);
  viewer.dataset.path = nextItem.path;
  viewer.dataset.kind = nextItem.kind;
  viewer.dataset.zoom = "1";
  viewer.dataset.offsetX = "0";
  viewer.dataset.offsetY = "0";
  if (title) title.textContent = mediaViewerFileName(nextItem.path);
  if (kind) kind.textContent = nextItem.kind === "video" ? "VIDEO" : "IMAGE";
  if (details) details.textContent = `${nextItem.path} · ${formatBytes(nextItem.size)}`;
  if (position) {
    position.textContent = items.length > 1 && index >= 0 ? `${index + 1} / ${items.length}` : "";
    position.classList.toggle("hidden", !(items.length > 1 && index >= 0));
  }

  const previous = viewer.querySelector("[data-media-viewer-previous]");
  const next = viewer.querySelector("[data-media-viewer-next]");
  const hasPrevious = index > 0;
  const hasNext = index >= 0 && index < items.length - 1;
  if (previous) previous.disabled = !hasPrevious;
  if (next) next.disabled = !hasNext;

  mediaViewerSetError(viewer, "");
  mediaViewerSetStatus(
    viewer,
    nextItem.kind === "video" ? "Loading video…" : "Loading image…",
  );
  image?.classList.toggle("hidden", nextItem.kind !== "image");
  video?.classList.toggle("hidden", nextItem.kind !== "video");
  image?.removeAttribute("src");
  if (video) {
    video.pause();
    video.removeAttribute("src");
    video.load();
  }
  if (nextItem.kind === "image") {
    if (image) {
      image.alt = nextItem.name || "Image preview";
      image.src = rawUrl;
    }
  } else if (video) {
    video.setAttribute("aria-label", nextItem.name || "Video preview");
    video.src = rawUrl;
    video.load();
  }
  mediaViewerApplyImageTransform(viewer);
}

function mediaViewerNavigate(viewer, direction) {
  if (!viewer) return;
  const items = mediaViewerItems();
  const index = items.findIndex((item) => item.path === viewer.dataset.path);
  const next = items[index + direction];
  if (!next) return;
  mediaViewerSetItem(viewer, next);
}

async function downloadMediaFile(filePath) {
  const project = activeProjectKey();
  if (!project || !filePath) return;
  const headers = state.token ? { Authorization: `Bearer ${state.token}` } : {};
  const response = await fetch(mediaViewerRawUrl(filePath, project), {
    cache: "no-store",
    headers,
  });
  if (!response.ok) {
    if (response.status === 401) showAuthPanel(authPanelMode());
    throw new Error(`Unable to download ${filePath}: ${response.statusText}`);
  }
  const blob = await response.blob();
  const objectUrl = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = objectUrl;
  link.download = mediaViewerFileName(filePath);
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(objectUrl), 1000);
}

function mediaViewerOpenOriginal(filePath) {
  const link = document.createElement("a");
  link.href = mediaViewerRawUrl(filePath);
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  link.referrerPolicy = "no-referrer";
  document.body.appendChild(link);
  link.click();
  link.remove();
}

function closeMediaViewer({ restoreFocus = true } = {}) {
  const viewer = qs("#media-viewer");
  if (!viewer) return;
  viewer.querySelector("[data-media-viewer-video]")?.pause();
  viewer.remove();
  document.body.classList.remove("media-viewer-open");
  const returnFocus = mediaViewerReturnFocus;
  mediaViewerReturnFocus = null;
  if (restoreFocus && returnFocus?.isConnected && !returnFocus.disabled) {
    window.requestAnimationFrame(() => returnFocus.focus({ preventScroll: true }));
  }
}

function mediaViewerHtml() {
  return `<div id="media-viewer" class="media-viewer" data-kind="image" data-zoom="1" data-offset-x="0" data-offset-y="0" role="presentation">
    <section class="media-viewer-dialog" role="dialog" aria-modal="true" aria-labelledby="media-viewer-title" aria-describedby="media-viewer-details">
      <header class="media-viewer-header">
        <div class="media-viewer-heading">
          <span class="media-viewer-kind" data-media-viewer-kind>IMAGE</span>
          <h2 id="media-viewer-title" data-media-viewer-title>Media preview</h2>
          <span id="media-viewer-details" class="media-viewer-details" data-media-viewer-details></span>
        </div>
        <div class="media-viewer-actions" aria-label="Media actions">
          <button type="button" class="icon-button" data-media-viewer-open aria-label="Open original" title="Open original" data-symbol="open"></button>
          <button type="button" class="icon-button" data-media-viewer-download aria-label="Download media" title="Download media" data-symbol="download"></button>
          <button type="button" class="icon-button" data-media-viewer-close aria-label="Close media viewer" title="Close" data-symbol="close"></button>
        </div>
      </header>
      <div class="media-viewer-stage" data-media-viewer-stage tabindex="0" aria-label="Media preview area">
        <div class="media-viewer-status" data-media-viewer-status role="status" aria-live="polite">Loading image…</div>
        <div class="media-viewer-error hidden" data-media-viewer-error role="alert"></div>
        <img class="media-viewer-image hidden" data-media-viewer-image alt="" draggable="false" />
        <video class="media-viewer-video hidden" data-media-viewer-video controls playsinline preload="metadata"></video>
      </div>
      <footer class="media-viewer-footer">
        <div class="media-viewer-navigation" aria-label="Media collection navigation">
          <button type="button" class="icon-button" data-media-viewer-previous aria-label="Previous media" title="Previous media" data-symbol="chevron-left"></button>
          <span class="media-viewer-position" data-media-viewer-position aria-live="polite"></span>
          <button type="button" class="icon-button" data-media-viewer-next aria-label="Next media" title="Next media" data-symbol="chevron-right"></button>
        </div>
        <div class="media-viewer-image-controls" aria-label="Image zoom controls">
          <button type="button" class="icon-button" data-media-viewer-zoom-out aria-label="Zoom out" title="Zoom out" data-symbol="zoom-out"></button>
          <span data-media-viewer-zoom-label>100%</span>
          <button type="button" class="icon-button" data-media-viewer-zoom-reset aria-label="Reset image" title="Reset image" data-symbol="maximize"></button>
          <button type="button" class="icon-button" data-media-viewer-zoom-in aria-label="Zoom in" title="Zoom in" data-symbol="zoom-in"></button>
        </div>
        <span class="media-viewer-help">Esc closes · ←/→ navigates · drag or pinch to explore</span>
      </footer>
    </section>
  </div>`;
}

function bindMediaViewer(viewer) {
  if (!viewer || mediaViewerBound) return;
  mediaViewerBound = true;
  const dialog = viewer.querySelector(".media-viewer-dialog");
  const stage = viewer.querySelector("[data-media-viewer-stage]");
  const image = viewer.querySelector("[data-media-viewer-image]");
  const video = viewer.querySelector("[data-media-viewer-video]");
  let pointerStart = null;
  let dragging = false;
  const activePointers = new Map();
  let pinchStartDistance = 0;
  let pinchStartZoom = 1;

  const pointerDistance = () => {
    const points = [...activePointers.values()];
    if (points.length < 2) return 0;
    return Math.hypot(points[0].x - points[1].x, points[0].y - points[1].y);
  };

  viewer.addEventListener("click", (event) => {
    if (event.target === viewer) {
      closeMediaViewer();
      return;
    }
    if (event.target.closest("[data-media-viewer-close]")) {
      closeMediaViewer();
    } else if (event.target.closest("[data-media-viewer-previous]")) {
      mediaViewerNavigate(viewer, -1);
    } else if (event.target.closest("[data-media-viewer-next]")) {
      mediaViewerNavigate(viewer, 1);
    } else if (event.target.closest("[data-media-viewer-zoom-out]")) {
      mediaViewerChangeZoom(viewer, -MEDIA_VIEWER_ZOOM_STEP);
    } else if (event.target.closest("[data-media-viewer-zoom-in]")) {
      mediaViewerChangeZoom(viewer, MEDIA_VIEWER_ZOOM_STEP);
    } else if (event.target.closest("[data-media-viewer-zoom-reset]")) {
      mediaViewerResetImageTransform(viewer);
    } else if (event.target.closest("[data-media-viewer-open]")) {
      mediaViewerOpenOriginal(viewer.dataset.path);
    } else if (event.target.closest("[data-media-viewer-download]")) {
      downloadMediaFile(viewer.dataset.path)
        .then(() => showToast("Media downloaded", "ok"))
        .catch(showError);
    }
  });
  viewer.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeMediaViewer();
      return;
    }
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      mediaViewerNavigate(viewer, -1);
      return;
    }
    if (event.key === "ArrowRight") {
      event.preventDefault();
      mediaViewerNavigate(viewer, 1);
      return;
    }
    if (event.key === "+" || event.key === "=") {
      event.preventDefault();
      mediaViewerChangeZoom(viewer, MEDIA_VIEWER_ZOOM_STEP);
      return;
    }
    if (event.key === "-") {
      event.preventDefault();
      mediaViewerChangeZoom(viewer, -MEDIA_VIEWER_ZOOM_STEP);
      return;
    }
    if (event.key === "0") {
      event.preventDefault();
      mediaViewerResetImageTransform(viewer);
      return;
    }
    trapMediaViewerFocus(event, dialog);
  });

  stage?.addEventListener("wheel", (event) => {
    if (viewer.dataset.kind !== "image" || (!event.ctrlKey && !event.metaKey)) return;
    event.preventDefault();
    mediaViewerChangeZoom(viewer, event.deltaY < 0 ? MEDIA_VIEWER_ZOOM_STEP : -MEDIA_VIEWER_ZOOM_STEP);
  }, { passive: false });
  stage?.addEventListener("dblclick", () => {
    if (viewer.dataset.kind !== "image") return;
    const zoom = Number(viewer.dataset.zoom || 1);
    if (zoom === 1) {
      viewer.dataset.zoom = "2";
      mediaViewerApplyImageTransform(viewer);
    } else {
      mediaViewerResetImageTransform(viewer);
    }
  });
  stage?.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    activePointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (activePointers.size === 2 && viewer.dataset.kind === "image") {
      pinchStartDistance = pointerDistance();
      pinchStartZoom = Number(viewer.dataset.zoom || 1);
      pointerStart = null;
      dragging = false;
      stage.setPointerCapture(event.pointerId);
      stage.classList.remove("is-dragging");
      return;
    }
    pointerStart = { x: event.clientX, y: event.clientY, time: Date.now() };
    dragging = viewer.dataset.kind === "image" && Number(viewer.dataset.zoom || 1) > 1;
    if (dragging) {
      stage.setPointerCapture(event.pointerId);
      stage.classList.add("is-dragging");
    }
  });
  stage?.addEventListener("pointermove", (event) => {
    if (activePointers.has(event.pointerId)) {
      activePointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    }
    if (activePointers.size >= 2 && pinchStartDistance > 0 && viewer.dataset.kind === "image") {
      const ratio = pointerDistance() / pinchStartDistance;
      const next = Math.min(
        MEDIA_VIEWER_MAX_ZOOM,
        Math.max(MEDIA_VIEWER_MIN_ZOOM, Math.round(pinchStartZoom * ratio * 100) / 100),
      );
      viewer.dataset.zoom = String(next);
      if (next === 1) {
        viewer.dataset.offsetX = "0";
        viewer.dataset.offsetY = "0";
      }
      mediaViewerApplyImageTransform(viewer);
      return;
    }
    if (!dragging || !pointerStart) return;
    const dx = event.clientX - pointerStart.x;
    const dy = event.clientY - pointerStart.y;
    viewer.dataset.offsetX = String(Number(viewer.dataset.offsetX || 0) + dx);
    viewer.dataset.offsetY = String(Number(viewer.dataset.offsetY || 0) + dy);
    pointerStart.x = event.clientX;
    pointerStart.y = event.clientY;
    mediaViewerApplyImageTransform(viewer);
  });
  const endPointer = (event) => {
    activePointers.delete(event.pointerId);
    const wasPinching = pinchStartDistance > 0;
    if (activePointers.size < 2) pinchStartDistance = 0;
    if (dragging) {
      stage.classList.remove("is-dragging");
      if (stage.hasPointerCapture(event.pointerId)) stage.releasePointerCapture(event.pointerId);
    } else if (!wasPinching && pointerStart && viewer.dataset.kind !== "video") {
      const dx = event.clientX - pointerStart.x;
      const dy = event.clientY - pointerStart.y;
      if (Math.abs(dx) >= MEDIA_VIEWER_SWIPE_DISTANCE && Math.abs(dy) <= MEDIA_VIEWER_SWIPE_VERTICAL_DRIFT) {
        mediaViewerNavigate(viewer, dx < 0 ? 1 : -1);
      }
    }
    pointerStart = null;
    dragging = false;
  };
  stage?.addEventListener("pointerup", endPointer);
  stage?.addEventListener("pointercancel", endPointer);
  stage?.addEventListener("pointerleave", (event) => {
    if (dragging && !stage.hasPointerCapture(event.pointerId)) endPointer(event);
  });
  image?.addEventListener("load", () => {
    if (viewer.dataset.kind !== "image") return;
    mediaViewerSetStatus(viewer, "", "ready");
    mediaViewerSetError(viewer, "");
  });
  image?.addEventListener("error", () => {
    if (viewer.dataset.kind !== "image") return;
    mediaViewerSetStatus(viewer, "", "error");
    mediaViewerSetError(viewer, "This image could not be loaded. It may be missing, unsupported, or larger than the server allows.");
  });
  video?.addEventListener("loadedmetadata", () => {
    if (viewer.dataset.kind === "video") mediaViewerSetStatus(viewer, "", "ready");
  });
  video?.addEventListener("loadeddata", () => {
    if (viewer.dataset.kind === "video") mediaViewerSetStatus(viewer, "", "ready");
  });
  video?.addEventListener("error", () => {
    if (viewer.dataset.kind !== "video") return;
    mediaViewerSetStatus(viewer, "", "error");
    mediaViewerSetError(viewer, "This video could not be played in the current browser. Try Open original to use a system player.");
  });
}

function openMediaViewer(filePath, kind = mediaViewerKindForPath(filePath)) {
  if (!kind) return false;
  if (!confirmDiscardDirtyFile()) return false;
  const project = activeProjectKey();
  if (!project) return false;
  closeFileContextMenu();
  closeMediaViewer({ restoreFocus: false });
  mediaViewerReturnFocus = document.activeElement instanceof HTMLElement && document.activeElement !== document.body
    ? document.activeElement
    : [...document.querySelectorAll("[data-file-open]")].find((button) => button.dataset.fileOpen === filePath) || null;
  document.body.insertAdjacentHTML("beforeend", mediaViewerHtml());
  const viewer = qs("#media-viewer");
  if (!viewer) return false;
  mediaViewerBound = false;
  bindMediaViewer(viewer);
  mediaViewerSetItem(viewer, {
    path: filePath,
    name: mediaViewerFileName(filePath),
    kind,
    size: findFileEntryByPath(state.fileEntries, filePath)?.size || 0,
  });
  document.body.classList.add("media-viewer-open");
  window.requestAnimationFrame(() => viewer.querySelector("[data-media-viewer-close]")?.focus({ preventScroll: true }));
  return true;
}
