// ==========================================================================
// FDM — Fast Download Manager · YouTube & Web Media Sniffer Overlay
// ==========================================================================

(() => {
  'use strict';

  if (window.__fdmSnifferLoaded) return;
  window.__fdmSnifferLoaded = true;

  let snifferEnabled = true;
  const isYouTube = () => /^(https?:\/\/)?([\w-]+\.)?(youtube\.com\/(watch|shorts|live)|youtu\.be\/)/i.test(location.href);

  // Load initial settings
  chrome.storage?.sync?.get('fdm.settings', (res) => {
    const settings = res?.['fdm.settings'] || {};
    if (settings.enabled === false || settings.mediaSniffer === false) {
      snifferEnabled = false;
      removeAllOverlays();
    }
  });

  chrome.storage?.onChanged?.addListener((changes, area) => {
    if (area === 'sync' && changes['fdm.settings']) {
      const settings = changes['fdm.settings'].newValue || {};
      snifferEnabled = settings.enabled !== false && settings.mediaSniffer !== false;
      if (!snifferEnabled) {
        removeAllOverlays();
      } else {
        init();
      }
    }
  });

  function removeAllOverlays() {
    document.querySelectorAll('#af-video-downloader, .fdm-media-pill').forEach(el => el.remove());
  }

  function cleanTitle() {
    let title = document.title || 'video';
    title = title.replace(/\s*-\s*YouTube$/i, '');
    title = title.replace(/[\\/:*?"<>|]/g, '_').trim();
    if (title.length > 90) title = title.substring(0, 90);
    return title || 'video';
  }

  // ========================================================================
  // 1. YouTube Specific Integration (IDM Style Player Overlay)
  // ========================================================================

  const YT_LEVELS = {
    highres: 4320, hd2880: 2880, hd2160: 2160, hd1440: 1440,
    hd1080: 1080, hd720: 720, large: 480, medium: 360, small: 240, tiny: 144
  };
  const YT_FALLBACK = [1080, 720, 480, 360];

  let ytRoot = null, ytPanel = null, ytHead = null, ytList = null, ytOpen = false;

  function buildYouTubeOverlay() {
    if (document.getElementById('af-video-downloader')) return;

    ytRoot = document.createElement('section');
    ytRoot.id = 'af-video-downloader';
    ytRoot.innerHTML = `
      <button id="af-toggle" title="Download with FDM" aria-label="Download with FDM">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 2L3 7V17L12 22L21 17V7L12 2Z" stroke="#e50914"/>
          <path d="M12 6V16M12 16L8 12M12 16L16 12"/>
        </svg>
        <i id="af-tbar" hidden></i>
      </button>
      <div id="af-panel" hidden>
        <div id="af-panel-head"><span id="af-head">Download Video</span></div>
        <div id="af-list"></div>
      </div>
    `;

    const toggle = ytRoot.querySelector('#af-toggle');
    ytPanel = ytRoot.querySelector('#af-panel');
    ytHead = ytRoot.querySelector('#af-head');
    ytList = ytRoot.querySelector('#af-list');

    toggle.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (ytOpen) {
        closeYouTubePanel();
      } else {
        openYouTubePanel();
      }
    });

    ytPanel.addEventListener('click', (e) => e.stopPropagation());
    document.addEventListener('click', (e) => {
      if (ytOpen && !ytRoot.contains(e.target)) closeYouTubePanel();
    });

    mountYouTubeOverlay();
  }

  function mountYouTubeOverlay() {
    if (!snifferEnabled || !isYouTube()) return;
    const player = document.querySelector('#movie_player') || document.querySelector('.html5-video-player');
    if (player && ytRoot && ytRoot.parentElement !== player) {
      player.appendChild(ytRoot);
    }
  }

  function openYouTubePanel() {
    ytOpen = true;
    ytPanel.hidden = false;
    renderYouTubeQualities();
  }

  function closeYouTubePanel() {
    ytOpen = false;
    if (ytPanel) ytPanel.hidden = true;
  }

  function getYouTubeAvailableQualities() {
    let levels = [];
    try {
      const player = document.getElementById('movie_player');
      if (player && typeof player.getAvailableQualityLevels === 'function') {
        levels = player.getAvailableQualityLevels();
      }
    } catch (e) {}

    const heights = [...new Set((levels || []).map(l => YT_LEVELS[l]).filter(Boolean))].sort((a, b) => b - a);
    const list = heights.length ? heights : YT_FALLBACK;

    const formats = list.map(h => ({
      kind: 'video',
      label: `${h}p HD`,
      height: h,
      ext: '.mp4',
    }));

    formats.push({
      kind: 'audio',
      label: 'Audio · MP3',
      ext: '.mp3',
    });

    return formats;
  }

  function renderYouTubeQualities() {
    if (!ytList) return;
    ytList.innerHTML = '';
    ytHead.textContent = 'Select Quality (FDM)';

    const formats = getYouTubeAvailableQualities();

    for (const fmt of formats) {
      const btn = document.createElement('button');
      btn.className = `af-item ${fmt.kind}`;
      btn.innerHTML = `
        <span class="af-label">${fmt.label}</span>
        <span class="af-size">${fmt.kind === 'audio' ? 'Audio' : 'Video'}</span>
      `;

      btn.addEventListener('click', () => {
        const title = cleanTitle();
        const filename = `${title} (${fmt.label.replace(' · ', '_')})${fmt.ext}`;

        closeYouTubePanel();

        // Send to FDM Native Host via Background
        chrome.runtime.sendMessage({
          type: 'downloadMedia',
          url: location.href,
          filename: filename,
          pageUrl: location.href,
        });
      });

      ytList.appendChild(btn);
    }
  }

  // ========================================================================
  // 2. Universal Web Media Sniffer (HTML5 video/audio on any page)
  // ========================================================================

  const detectedMedia = new Map();

  function getMediaSrc(el) {
    if (el.currentSrc && !el.currentSrc.startsWith('blob:')) return el.currentSrc;
    if (el.src && !el.src.startsWith('blob:')) return el.src;
    const source = el.querySelector('source[src]');
    if (source && source.src && !source.src.startsWith('blob:')) return source.src;
    return null;
  }

  function attachUniversalPill(mediaEl, srcUrl) {
    if (!snifferEnabled || isYouTube()) return;
    if (detectedMedia.has(mediaEl)) return;

    const isAudio = mediaEl.tagName.toLowerCase() === 'audio';
    const ext = isAudio ? '.mp3' : '.mp4';
    const label = isAudio ? 'Download Audio' : 'Download Video';

    const pill = document.createElement('div');
    pill.className = 'fdm-media-pill';
    pill.innerHTML = `
      <svg viewBox="0 0 24 24">
        <path d="M12 2L3 7V17L12 22L21 17V7L12 2Z"></path>
        <path d="M12 6V16M12 16L8 12M12 16L16 12"></path>
      </svg>
      <span class="fdm-media-text">${label}</span>
    `;

    function updatePos() {
      if (!mediaEl.isConnected) {
        pill.remove();
        detectedMedia.delete(mediaEl);
        return;
      }
      const rect = mediaEl.getBoundingClientRect();
      if (rect.width < 50 || rect.height < 50) {
        pill.style.display = 'none';
        return;
      }

      pill.style.display = 'inline-flex';
      const top = Math.max(8, window.scrollY + rect.top + 8);
      const left = Math.max(8, window.scrollX + rect.right - 140);
      pill.style.top = `${top}px`;
      pill.style.left = `${left}px`;
    }

    pill.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();

      const url = srcUrl || getMediaSrc(mediaEl) || location.href;
      const filename = cleanTitle() + ext;

      pill.classList.add('fdm-pill-downloading');
      pill.querySelector('.fdm-media-text').textContent = 'Opening in FDM…';

      chrome.runtime.sendMessage({
        type: 'downloadMedia',
        url: url,
        filename: filename,
        pageUrl: location.href,
      }, () => {
        setTimeout(() => {
          pill.classList.remove('fdm-pill-downloading');
          pill.querySelector('.fdm-media-text').textContent = label;
        }, 2500);
      });
    });

    document.body.appendChild(pill);
    detectedMedia.set(mediaEl, pill);
    updatePos();

    window.addEventListener('resize', updatePos, { passive: true });
    window.addEventListener('scroll', updatePos, { passive: true });
  }

  function scanGeneralMedia() {
    if (!snifferEnabled || isYouTube()) return;
    document.querySelectorAll('video, audio').forEach((el) => {
      const src = getMediaSrc(el);
      if (src) attachUniversalPill(el, src);
      el.addEventListener('play', () => {
        const s = getMediaSrc(el);
        if (s) attachUniversalPill(el, s);
      }, { passive: true });
    });
  }

  // ========================================================================
  // Initialization & Dynamic Navigation Observer
  // ========================================================================

  function init() {
    if (!snifferEnabled) return;
    if (isYouTube()) {
      buildYouTubeOverlay();
    } else {
      scanGeneralMedia();
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }

  new MutationObserver(() => {
    if (!snifferEnabled) return;
    if (isYouTube()) {
      mountYouTubeOverlay();
    } else {
      scanGeneralMedia();
    }
  }).observe(document.documentElement, { childList: true, subtree: true });

  window.addEventListener('yt-navigate-finish', () => {
    if (isYouTube()) {
      closeYouTubePanel();
      mountYouTubeOverlay();
    }
  });

  setInterval(() => {
    if (isYouTube() && snifferEnabled) {
      mountYouTubeOverlay();
    }
  }, 2000);
})();
