// ==========================================================================
// FDM — Fast Download Manager · YouTube & Web Media Sniffer Overlay
// ==========================================================================

(() => {
  'use strict';

  if (window.__fdmSnifferLoaded) return;
  window.__fdmSnifferLoaded = true;

  let snifferEnabled = true;

  function isYouTube() {
    return window.location.hostname.includes('youtube.com') || window.location.hostname.includes('youtu.be');
  }

  function isYouTubeWatch() {
    if (!isYouTube()) return false;
    const path = window.location.pathname;
    const search = window.location.search;
    return path.includes('/watch') || path.includes('/shorts') || path.includes('/live') || path.includes('/embed') || search.includes('v=');
  }

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
    document.querySelectorAll('#af-video-downloader, .fdm-media-pill, #fdm-sniff-bar').forEach(el => el.remove());
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

  let ytRoot = null, ytPanel = null, ytHead = null, ytList = null, ytOpen = false;
  const qualitiesCache = new Map();

  function getVideoId() {
    try {
      const u = new URL(location.href);
      return u.searchParams.get('v') || u.pathname.split('/').pop() || location.href;
    } catch (_) {
      return location.href;
    }
  }

  function buildYouTubeOverlay() {
    const existing = document.getElementById('af-video-downloader');
    if (existing) {
      ytRoot = existing;
      ytPanel = ytRoot.querySelector('#af-panel');
      ytHead = ytRoot.querySelector('#af-head');
      ytList = ytRoot.querySelector('#af-list');
      return;
    }

    ytRoot = document.createElement('section');
    ytRoot.id = 'af-video-downloader';
    ytRoot.innerHTML = `
      <button id="af-toggle" title="Download this video with FDM" aria-label="Download with FDM">
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 2L3 7V17L12 22L21 17V7L12 2Z" stroke="#e50914" fill="#e50914" fill-opacity="0.25"/>
          <path d="M12 7V15M12 15L9 12M12 15L15 12" stroke="#ffffff"/>
        </svg>
        <span class="af-toggle-text">Download</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
          <path d="M6 9l6 6 6-6"/>
        </svg>
        <i id="af-tbar" hidden></i>
      </button>
      <div id="af-panel" hidden>
        <div id="af-panel-head"><span id="af-head">Select Quality</span></div>
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
      if (ytOpen && ytRoot && !ytRoot.contains(e.target)) closeYouTubePanel();
    });

    mountYouTubeOverlay();
  }

  function mountYouTubeOverlay() {
    if (!snifferEnabled || !isYouTubeWatch()) return;
    if (!ytRoot) buildYouTubeOverlay();

    const player = document.querySelector('#movie_player') ||
                   document.querySelector('.html5-video-player') ||
                   document.querySelector('ytd-player') ||
                   document.querySelector('#player');

    if (player && ytRoot && ytRoot.parentElement !== player) {
      player.appendChild(ytRoot);
    }
  }

  async function openYouTubePanel() {
    ytOpen = true;
    if (ytPanel) ytPanel.hidden = false;
    await renderYouTubeQualities();
  }

  function closeYouTubePanel() {
    ytOpen = false;
    if (ytPanel) ytPanel.hidden = true;
  }

  async function queryQualitiesOnce() {
    const heightsMap = new Map(); // height -> { height, directUrl }
    let audioDirectUrl = null;

    try {
      const res = await new Promise((resolve) => {
        chrome.runtime.sendMessage({ type: 'pageQualities' }, (r) => {
          resolve(chrome.runtime.lastError ? null : r);
        });
      });

      if (res && res.ok) {
        if (Array.isArray(res.levels)) {
          res.levels.forEach(l => {
            if (YT_LEVELS[l]) {
              const h = YT_LEVELS[l];
              if (!heightsMap.has(h)) heightsMap.set(h, { height: h });
            }
          });
        }
        if (Array.isArray(res.qualityData)) {
          res.qualityData.forEach(d => {
            const h = d.height || (d.quality ? YT_LEVELS[d.quality] : null);
            if (h && !heightsMap.has(h)) heightsMap.set(h, { height: h });
          });
        }
        if (Array.isArray(res.streamFormats)) {
          res.streamFormats.forEach(f => {
            let h = f.height;
            if (!h && f.qualityLabel) {
              const m = f.qualityLabel.match(/(\d+)p/i);
              if (m) h = parseInt(m[1], 10);
            }
            if (h) {
              const existing = heightsMap.get(h) || { height: h };
              if (f.url && !existing.directUrl) existing.directUrl = f.url;
              heightsMap.set(h, existing);
            } else if (f.mimeType && f.mimeType.includes('audio') && f.url && !audioDirectUrl) {
              audioDirectUrl = f.url;
            }
          });
        }
      }
    } catch (_) {}

    // Direct player inspect fallback
    try {
      const player = document.getElementById('movie_player') || document.querySelector('.html5-video-player');
      if (player && typeof player.getAvailableQualityLevels === 'function') {
        const levels = player.getAvailableQualityLevels();
        if (Array.isArray(levels)) {
          levels.forEach(l => {
            if (YT_LEVELS[l]) {
              const h = YT_LEVELS[l];
              if (!heightsMap.has(h)) heightsMap.set(h, { height: h });
            }
          });
        }
      }
    } catch (_) {}

    return { heightsMap, audioDirectUrl };
  }

  async function getYouTubeAvailableQualities() {
    const videoId = getVideoId();
    if (qualitiesCache.has(videoId)) {
      return qualitiesCache.get(videoId);
    }

    let { heightsMap, audioDirectUrl } = await queryQualitiesOnce();

    // If still empty (video player bootstrapping), retry
    if (heightsMap.size === 0) {
      for (let i = 0; i < 4; i++) {
        await new Promise(r => setTimeout(r, 200));
        const res = await queryQualitiesOnce();
        heightsMap = res.heightsMap;
        if (!audioDirectUrl) audioDirectUrl = res.audioDirectUrl;
        if (heightsMap.size > 0) break;
      }
    }

    let heights = [...heightsMap.keys()].filter(h => h >= 144 && h <= 4320).sort((a, b) => b - a);

    if (!heights.length) {
      heights = [1080, 720, 480, 360];
      heights.forEach(h => heightsMap.set(h, { height: h }));
    }

    const formats = heights.map(h => {
      let label = `${h}p`;
      let badge = 'HD';
      if (h >= 4320) { label = '4320p (8K)'; badge = '8K UHD'; }
      else if (h >= 2160) { label = '2160p (4K)'; badge = '4K UHD'; }
      else if (h >= 1440) { label = '1440p (2K)'; badge = '2K QHD'; }
      else if (h >= 1080) { label = '1080p (FHD)'; badge = '1080p'; }
      else if (h >= 720) { label = '720p (HD)'; badge = '720p'; }
      else { label = `${h}p (SD)`; badge = `${h}p`; }

      const item = heightsMap.get(h) || {};
      return { kind: 'video', label: label, badge: badge, height: h, ext: '.mp4', directUrl: item.directUrl || null };
    });

    formats.push({ kind: 'audio', label: 'Audio · MP3', badge: 'Audio', ext: '.mp3', directUrl: audioDirectUrl || null });

    if (heightsMap.size > 0 && videoId) {
      qualitiesCache.set(videoId, formats);
    }
    return formats;
  }

  async function renderYouTubeQualities() {
    if (!ytList) return;
    const videoId = getVideoId();
    if (!qualitiesCache.has(videoId)) {
      ytList.innerHTML = '<div class="af-msg">Reading available qualities…</div>';
    }
    if (ytHead) ytHead.textContent = 'Select Quality';

    const formats = await getYouTubeAvailableQualities();
    if (!ytOpen || !ytList) return;

    ytList.innerHTML = '';
    for (const fmt of formats) {
      const btn = document.createElement('button');
      btn.className = `af-item ${fmt.kind}`;
      btn.innerHTML = `
        <span class="af-label">${fmt.label}</span>
        <span class="af-size">${fmt.kind === 'audio' ? 'MP3' : fmt.badge}</span>
      `;

      btn.addEventListener('click', () => {
        const title = cleanTitle();
        const filename = `${title} (${fmt.label.replace(' · ', '_')})${fmt.ext}`;

        closeYouTubePanel();

        let targetUrl = location.href;
        try {
          const u = new URL(location.href);
          if (u.hostname.includes('youtube.com') || u.hostname.includes('youtu.be')) {
            const v = u.searchParams.get('v');
            if (v) targetUrl = `https://www.youtube.com/watch?v=${v}`;
          }
        } catch (_) {}

        // Send to FDM Native Host via Background (direct URL if available for instant connection)
        chrome.runtime.sendMessage({
          type: 'downloadMedia',
          url: fmt.directUrl || targetUrl,
          pageUrl: location.href,
          filename: filename,
        });
      });

      ytList.appendChild(btn);
    }
  }

  // ========================================================================
  // 2. Universal Web Media Sniffer (HTML5 video/audio on other pages)
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

      const title = cleanTitle();
      const filename = `${title}${ext}`;

      pill.classList.add('fdm-pill-downloading');
      pill.querySelector('.fdm-media-text').textContent = 'Starting...';

      chrome.runtime.sendMessage({
        type: 'downloadMedia',
        url: srcUrl,
        pageUrl: location.href,
        filename: filename,
      }, () => {
        setTimeout(() => {
          pill.classList.remove('fdm-pill-downloading');
          pill.querySelector('.fdm-media-text').textContent = label;
        }, 2000);
      });
    });

    document.body.appendChild(pill);
    detectedMedia.set(mediaEl, pill);
    updatePos();

    window.addEventListener('scroll', updatePos, { passive: true });
    window.addEventListener('resize', updatePos, { passive: true });
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

  // Lifecycle
  let activeVideoId = getVideoId();

  function checkVideoIdChange() {
    if (!isYouTube()) return;
    const currentVideoId = getVideoId();
    if (isYouTubeWatch()) {
      mountYouTubeOverlay();
      if (currentVideoId && currentVideoId !== activeVideoId) {
        if (activeVideoId) qualitiesCache.delete(activeVideoId);
        qualitiesCache.delete(currentVideoId);
        activeVideoId = currentVideoId;
        closeYouTubePanel();
        if (ytList) ytList.innerHTML = '<div class="af-msg">Reading available qualities…</div>';
        setTimeout(() => {
          getYouTubeAvailableQualities().catch(() => {});
        }, 150);
      }
    }
  }

  function setupPlayerVideoListeners() {
    const video = document.querySelector('video');
    if (video && !video.__fdmListenerAttached) {
      video.__fdmListenerAttached = true;
      video.addEventListener('playing', () => {
        checkVideoIdChange();
        getYouTubeAvailableQualities().catch(() => {});
      });
      video.addEventListener('loadeddata', () => {
        checkVideoIdChange();
        getYouTubeAvailableQualities().catch(() => {});
      });
    }
  }

  function init() {
    if (!snifferEnabled) return;
    if (isYouTube()) {
      if (isYouTubeWatch()) {
        buildYouTubeOverlay();
        setupPlayerVideoListeners();
        getYouTubeAvailableQualities().catch(() => {});
      }
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
      checkVideoIdChange();
      setupPlayerVideoListeners();
    } else {
      scanGeneralMedia();
    }
  }).observe(document.documentElement, { childList: true, subtree: true });

  window.addEventListener('yt-navigate-finish', () => {
    checkVideoIdChange();
    mountYouTubeOverlay();
    setupPlayerVideoListeners();
    setTimeout(() => {
      getYouTubeAvailableQualities().catch(() => {});
    }, 200);
  });
  window.addEventListener('yt-page-data-updated', () => {
    checkVideoIdChange();
    mountYouTubeOverlay();
    setupPlayerVideoListeners();
  });
  window.addEventListener('popstate', () => {
    checkVideoIdChange();
  });

  setInterval(() => {
    if (isYouTube() && snifferEnabled) {
      checkVideoIdChange();
      mountYouTubeOverlay();
      setupPlayerVideoListeners();
    }
  }, 1000);
})();
