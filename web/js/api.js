// The only module that talks to the kimo server. Token from #token=… is kept in localStorage.
const api = (() => {
  const KEY = 'kimo-token';
  const read = () => { try { return localStorage.getItem(KEY) || ''; } catch { return ''; } };
  const m = location.hash.match(/token=([\w-]+)/);
  if (m) {
    try { localStorage.setItem(KEY, m[1]); } catch { /* private mode: token lives for this page only */ }
    history.replaceState(null, '', location.pathname);
  }
  let token = m ? m[1] : read();

  class AuthError extends Error {}

  async function call(method, path, body) {
    const res = await fetch(`/api${path}`, {
      method,
      headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (res.status === 401) throw new AuthError('Token needed');
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    return data;
  }

  return {
    AuthError,
    setToken(t) { token = t; try { localStorage.setItem(KEY, t); } catch { /* ignore */ } },
    status: () => call('GET', '/status'),
    sessions: () => call('GET', '/sessions'),
    createSession: body => call('POST', '/sessions', body),
    deleteSession: id => call('DELETE', `/sessions/${id}`),
    act: (id, action) => call('POST', `/sessions/${id}/action`, { action }),
    setAccount: (id, account) => call('POST', `/sessions/${id}/account`, { account }),
    accounts: () => call('GET', '/accounts'),
    createAccount: body => call('POST', '/accounts', body),
    deleteAccount: id => call('DELETE', `/accounts/${id}`),
    login: id => call('POST', `/accounts/${id}/login`),
    moveSessions: (from, to) => call('POST', `/accounts/${from}/move`, { to }),
    termUrl(id) {
      const proto = location.protocol === 'https:' ? 'wss' : 'ws';
      return `${proto}://${location.host}/api/sessions/${id}/term${token ? `?token=${encodeURIComponent(token)}` : ''}`;
    },
  };
})();
