// Homepage-only language hint (todo.md line 347, option B).
//
// A first-time visitor whose browser reports a Chinese language gets sent to
// the localized homepage; every other path keeps the current locale. The
// probe runs ONLY on the English root homepage and ONLY once per browser
// session, so a deliberate language switch (or a deep link into content) is
// never overridden.
//
// The decision logic is a pure function (localeRedirectTarget) so the
// contract is unit-testable without a browser.

// Returns the site path the homepage visitor should be sent to, or null to
// stay put. `base` is the VitePress site base (e.g. "/custos/") and is
// honored so the redirect stays inside this deployment.
export function localeRedirectTarget(base, lang) {
  const language = (lang || '').toLowerCase()
  if (!language.startsWith('zh')) {
    return null
  }
  return (base || '/').replace(/\/+$/, '') + '/zh/'
}

// Returns true when `path` is the English root homepage (the site base),
// where the locale hint is allowed to run. Every other path — localized
// pages, guides, ADRs — is left untouched.
export function isEnglishHome(path, base) {
  const normalize = (p) => (String(p || '').replace(/\/+$/, '') || '/') + '/'
  return normalize(path) === normalize(base || '/')
}

// Wire the homepage redirect onto the running VitePress app. `storage` and
// `location` are injected so this is testable without a browser.
export function maybeRedirectHome({ route, lang, base, storage, location }) {
  if (!isEnglishHome(route.path, base)) {
    return null
  }
  // One shot per browser session; a deliberate language switch or a deep
  // link into content is never overridden by this hint.
  if (storage.getItem('custos:locale-redirected')) {
    return null
  }
  const target = localeRedirectTarget(base, lang)
  storage.setItem('custos:locale-redirected', target ? 'zh' : 'seen')
  if (target) {
    location.assign(target)
  }
  return target
}
