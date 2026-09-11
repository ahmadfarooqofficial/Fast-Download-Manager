// ==========================================================================
// FDM — dragging a finished file out of the app.
//
// The obvious approach, an HTML5 `dragstart` that sets `text/uri-list`, does
// not work: a webview drag carries text, and Explorer only accepts a drop that
// carries CF_HDROP from a real OLE drag source. So we cancel the browser's drag
// and ask the Rust side to start a native one in its place — which is what IDM
// is doing when you drag a finished download onto your desktop.
// ==========================================================================
(function () {
  const tauri = window.__TAURI__ || {};
  const invoke =
    (tauri.core && tauri.core.invoke) || tauri.invoke ||
    (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
  const Channel = tauri.core && tauri.core.Channel;

  /// The picture that follows the cursor during the drag.
  ///
  /// Drawn rather than loaded: the plugin wants a PNG data URL, and generating
  /// one here avoids shipping an image purely to be encoded back into the same
  /// bytes at runtime. Without it the shell drags an empty rectangle.
  let cachedIcon = null;
  function dragImage() {
    if (cachedIcon) return cachedIcon;
    const size = 64;
    const c = document.createElement('canvas');
    c.width = size;
    c.height = size;
    const ctx = c.getContext('2d');

    // A page with a folded corner, in the brand red on a dark card.
    ctx.fillStyle = '#0e1223';
    ctx.strokeStyle = '#e50914';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.roundRect(10, 6, 44, 52, 6);
    ctx.fill();
    ctx.stroke();

    ctx.fillStyle = '#e50914';
    ctx.beginPath();
    ctx.moveTo(38, 6);
    ctx.lineTo(54, 22);
    ctx.lineTo(38, 22);
    ctx.closePath();
    ctx.fill();

    ctx.strokeStyle = '#94a3b8';
    ctx.lineWidth = 3;
    ctx.lineCap = 'round';
    for (const y of [34, 42, 50]) {
      ctx.beginPath();
      ctx.moveTo(19, y);
      ctx.lineTo(y === 50 ? 36 : 45, y);
      ctx.stroke();
    }

    cachedIcon = c.toDataURL('image/png');
    return cachedIcon;
  }

  /// Start a native drag of `path`. Call from a `dragstart` handler after
  /// `preventDefault()`.
  async function startFileDrag(path) {
    if (!path || !invoke) return;
    try {
      await invoke('plugin:drag|start_drag', {
        // The plugin's DragItem is untagged: a bare array is a file list.
        item: [path],
        image: dragImage(),
        // Copy, not move — dragging a download somewhere should never be the
        // reason it disappears from where the user filed it.
        options: { mode: 'copy' },
        // Required by the command signature even when we ignore the result.
        onEvent: Channel ? new Channel() : undefined,
      });
    } catch (err) {
      console.error('Native drag-out failed:', err);
    }
  }

  /// Wire an element so it drags `pathFor(el)` out of the app.
  function makeDraggable(el, pathFor) {
    if (!el) return;
    el.setAttribute('draggable', 'true');
    el.addEventListener('dragstart', (e) => {
      const path = pathFor(e.currentTarget);
      if (!path) return;
      // Stop the webview from starting its own (useless) text drag.
      e.preventDefault();
      startFileDrag(path);
    });
  }

  window.fdmDragOut = { startFileDrag, makeDraggable };
})();
