// Contract tests for the homepage-only locale hint (todo.md line 347 B).
// Run: node docs/.vitepress/theme/localeRedirect.test.mjs
//
// The contract has three parts:
//   1. A Chinese browser on the English root homepage is sent to /zh/.
//   2. Any non-Chinese browser on the English root homepage stays put.
//   3. A redirect NEVER fires on a path that is not the English root
//      homepage (localized pages, guides, ADRs, deep links) — and never
//      twice in one browser session.
import assert from 'node:assert'
import { maybeRedirectHome, localeRedirectTarget, isEnglishHome } from './localeRedirect.ts'

const BASE = '/custos/'

// --- pure decision ----------------------------------------------------------
assert.equal(localeRedirectTarget(BASE, 'zh-CN'), '/custos/zh/')
assert.equal(localeRedirectTarget(BASE, 'zh'), '/custos/zh/')
assert.equal(localeRedirectTarget(BASE, 'ZH-tw'), '/custos/zh/')
assert.equal(localeRedirectTarget(BASE, 'en-US'), null)
assert.equal(localeRedirectTarget(BASE, ''), null)
assert.equal(localeRedirectTarget(BASE, undefined), null)
assert.equal(localeRedirectTarget('/', 'zh'), '/zh/')
// base is honored so the redirect stays inside this deployment
assert.equal(localeRedirectTarget('/custos/', 'zh-CN'), '/custos/zh/')
assert.equal(localeRedirectTarget('', 'zh-CN'), '/zh/')

// --- path gating ------------------------------------------------------------
assert.ok(isEnglishHome('/custos/', BASE))
assert.ok(isEnglishHome('/custos', BASE))
assert.ok(isEnglishHome('/', '/'))
assert.ok(!isEnglishHome('/custos/zh/', BASE))
assert.ok(!isEnglishHome('/custos/guide/overview', BASE))
assert.ok(!isEnglishHome('/custos/adr/ADR-0001-why-rust', BASE))
assert.ok(!isEnglishHome('/custos/zh/guide/overview', BASE))

// --- wired behavior ---------------------------------------------------------
function makeDeps({ alreadyRedirected = null } = {}) {
  const assigned = []
  const store = new Map()
  if (alreadyRedirected !== null) store.set('custos:locale-redirected', alreadyRedirected)
  return {
    assigned,
    storage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, v),
    },
    location: { assign: (to) => assigned.push(to) },
  }
}

// 1. Chinese browser on the English root homepage is sent to /zh/ once.
{
  const deps = makeDeps()
  const target = maybeRedirectHome({ route: { path: '/custos/' }, lang: 'zh-CN', base: BASE, ...deps })
  assert.equal(target, '/custos/zh/')
  assert.deepEqual(deps.assigned, ['/custos/zh/'])
  // Second call in the same session does not redirect again.
  const again = maybeRedirectHome({ route: { path: '/custos/' }, lang: 'zh-CN', base: BASE, ...deps })
  assert.equal(again, null)
  assert.deepEqual(deps.assigned, ['/custos/zh/'])
}

// 2. Non-Chinese browser on the English root homepage stays put, but the
//    decision is recorded so a later in-session navigation home does not
//    re-probe.
{
  const deps = makeDeps()
  const target = maybeRedirectHome({ route: { path: '/custos/' }, lang: 'en-US', base: BASE, ...deps })
  assert.equal(target, null)
  assert.deepEqual(deps.assigned, [])
  assert.equal(deps.storage.getItem('custos:locale-redirected'), 'seen')
}

// 3. A localized or deep-linked path never redirects, regardless of language.
for (const path of ['/custos/zh/', '/custos/guide/overview', '/custos/zh/guide/overview', '/custos/adr/ADR-0001-why-rust']) {
  const deps = makeDeps()
  const target = maybeRedirectHome({ route: { path }, lang: 'zh-CN', base: BASE, ...deps })
  assert.equal(target, null, `no redirect for ${path}`)
  assert.deepEqual(deps.assigned, [])
  assert.equal(deps.storage.getItem('custos:locale-redirected'), null, 'no decision recorded off-homepage')
}

// A non-English homepage path that merely ends in the base is still not the
// English root and must not redirect.
{
  const deps = makeDeps()
  assert.equal(maybeRedirectHome({ route: { path: '/custos/guide/overview' }, lang: 'zh-CN', base: BASE, ...deps }), null)
  assert.deepEqual(deps.assigned, [])
}

console.log('localeRedirect contract: all assertions passed')
