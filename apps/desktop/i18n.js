// ==========================================================================
// FDM — minimal i18n loader (no framework/bundler, so this is plain JS).
// Loaded before app.js / download_dialog.js so window.fdmI18n is ready.
// The dictionaries themselves are set by i18n/en.js and i18n/ru.js.
// ==========================================================================
(function () {
  const STORAGE_KEY = 'fdm_language';
  const SUPPORTED = ['en', 'ru'];

  function detectLanguage() {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved && SUPPORTED.includes(saved)) return saved;
    } catch (_err) {
      // localStorage unavailable — fall through to system locale.
    }
    const nav = (navigator.language || 'en').toLowerCase();
    return nav.startsWith('ru') ? 'ru' : 'en';
  }

  function dictFor(lang) {
    return lang === 'ru' ? (window.FDM_I18N_RU || {}) : (window.FDM_I18N_EN || {});
  }

  let currentLang = detectLanguage();

  // {n} style placeholder substitution, e.g. t('dialog_status_downloading', {n: 8}).
  function t(key, vars) {
    const raw = dictFor(currentLang)[key] || (window.FDM_I18N_EN || {})[key] || key;
    if (!vars) return raw;
    return raw.replace(/\{(\w+)\}/g, (m, name) => (name in vars ? String(vars[name]) : m));
  }

  function applyTranslations(root) {
    root = root || document;
    root.querySelectorAll('[data-i18n]').forEach((el) => {
      el.textContent = t(el.getAttribute('data-i18n'));
    });
    root.querySelectorAll('[data-i18n-html]').forEach((el) => {
      el.innerHTML = t(el.getAttribute('data-i18n-html'));
    });
    root.querySelectorAll('[data-i18n-title]').forEach((el) => {
      el.title = t(el.getAttribute('data-i18n-title'));
    });
    root.querySelectorAll('[data-i18n-aria-label]').forEach((el) => {
      el.setAttribute('aria-label', t(el.getAttribute('data-i18n-aria-label')));
    });
    root.querySelectorAll('[data-i18n-placeholder]').forEach((el) => {
      el.setAttribute('placeholder', t(el.getAttribute('data-i18n-placeholder')));
    });
    document.documentElement.lang = currentLang;
  }

  function setLanguage(lang) {
    if (!SUPPORTED.includes(lang) || lang === currentLang) return;
    currentLang = lang;
    try {
      localStorage.setItem(STORAGE_KEY, lang);
    } catch (_err) {
      // Non-fatal — the choice just won't persist across restarts.
    }
    applyTranslations();
    document.dispatchEvent(new CustomEvent('fdm-language-changed', { detail: { lang } }));
  }

  window.fdmI18n = {
    t,
    applyTranslations,
    setLanguage,
    getLanguage: () => currentLang,
    supportedLanguages: SUPPORTED.slice(),
  };

  document.addEventListener('DOMContentLoaded', () => applyTranslations());

  // All windows (main + per-download popups) share the same origin/storage,
  // so a language change in one window's Settings reaches the others via the
  // standard cross-window `storage` event — no Tauri IPC needed.
  window.addEventListener('storage', (e) => {
    if (e.key === STORAGE_KEY && e.newValue && SUPPORTED.includes(e.newValue) && e.newValue !== currentLang) {
      currentLang = e.newValue;
      applyTranslations();
      document.dispatchEvent(new CustomEvent('fdm-language-changed', { detail: { lang: currentLang } }));
    }
  });
})();
