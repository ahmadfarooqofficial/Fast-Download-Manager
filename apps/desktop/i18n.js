// ==========================================================================
// FDM — minimal i18n runtime (no framework/bundler, so this is plain JS).
// Loaded before app.js / download_dialog.js so window.fdmI18n is ready.
// The dictionaries themselves live in i18n/languages.js.
// ==========================================================================
(function () {
  const STORAGE_KEY = 'fdm_language';
  const DICTS = window.FDM_I18N || {};
  const LANGUAGES = window.FDM_LANGUAGES || [{ code: 'en', name: 'English' }];
  const SUPPORTED = LANGUAGES.map((l) => l.code);

  function detectLanguage() {
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved && SUPPORTED.includes(saved)) return saved;
    } catch (_err) {
      // localStorage unavailable — fall through to the system locale.
    }
    // "pt-BR" should find "pt"; an unknown locale falls back to English.
    const tag = (navigator.language || 'en').toLowerCase();
    const base = tag.split('-')[0];
    return SUPPORTED.includes(base) ? base : 'en';
  }

  let currentLang = detectLanguage();

  // {n} style placeholders, e.g. t('dialog_status_downloading', {n: 8}).
  function t(key, vars) {
    const raw = (DICTS[currentLang] && DICTS[currentLang][key]) || (DICTS.en && DICTS.en[key]) || key;
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

    const meta = LANGUAGES.find((l) => l.code === currentLang);
    document.documentElement.lang = currentLang;
    // Arabic and Urdu read right to left. The layout is not mirrored beyond
    // what the browser does from `dir` alone, but text, inputs and punctuation
    // all land the right way round, which is what makes it readable.
    document.documentElement.dir = meta && meta.rtl ? 'rtl' : 'ltr';
  }

  function setLanguage(lang) {
    if (!SUPPORTED.includes(lang) || lang === currentLang) return;
    currentLang = lang;
    try {
      localStorage.setItem(STORAGE_KEY, lang);
    } catch (_err) {
      // Non-fatal — the choice just won't survive a restart.
    }
    applyTranslations();
    document.dispatchEvent(new CustomEvent('fdm-language-changed', { detail: { lang } }));
  }

  /// Fill a <select> with every available language, marking the current one.
  function populateSelect(select) {
    if (!select) return;
    select.innerHTML = LANGUAGES.map(
      (l) => `<option value="${l.code}">${l.name}</option>`
    ).join('');
    select.value = currentLang;
  }

  window.fdmI18n = {
    t,
    applyTranslations,
    setLanguage,
    populateSelect,
    getLanguage: () => currentLang,
    languages: LANGUAGES,
  };

  document.addEventListener('DOMContentLoaded', () => applyTranslations());

  // All windows (main + per-download popups) share an origin and therefore
  // localStorage, so a change in one window's Settings reaches the others
  // through the standard cross-window `storage` event — no Tauri IPC needed.
  window.addEventListener('storage', (e) => {
    if (e.key === STORAGE_KEY && e.newValue && SUPPORTED.includes(e.newValue) && e.newValue !== currentLang) {
      currentLang = e.newValue;
      applyTranslations();
      document.dispatchEvent(new CustomEvent('fdm-language-changed', { detail: { lang: currentLang } }));
    }
  });
})();
