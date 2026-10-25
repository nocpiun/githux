import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
const exec = promisify(execFile);

function response(raw) {
  const text = String(raw ?? '').replace(/\r\n/g, '\n');
  const split = text.indexOf('\n\n');
  const status = /^HTTP\/\S+ (\d{3})\b/.exec(text);
  if (!status || split < 0) throw new Error('Missing gh HTTP response headers');
  const headers = Object.fromEntries(text.slice(0, split).split('\n').slice(1).map(line => {
    const i = line.indexOf(':');
    return [line.slice(0, i).trim().toLowerCase(), line.slice(i + 1).trim()];
  }));
  return { status: Number(status[1]), headers, body: text.slice(split + 2) };
}

function nextPage(link) {
  const match = /<([^>]+)>;\s*rel="next"/.exec(link ?? '');
  if (!match) return null;
  const url = new URL(match[1]);
  if (url.origin !== 'https://api.github.com' || url.username || url.password || !url.pathname.startsWith('/repos/'))
    throw new Error('Untrusted pagination URL');
  return url.pathname.slice(1) + url.search;
}

export function createGithubApi({ execute = exec, now = Date.now } = {}) {
  const cache = new Map();
  return async function api(endpoint, { paginate = false, collection, query, signal,
    text = false, immutable = false } = {}) {
    if (query && (endpoint !== 'graphql' || !/^query\b/.test(query)))
      throw new Error('Only read-only GraphQL queries are supported');
    if (!/^(?:repos\/|graphql$)/.test(endpoint) || endpoint.includes('://'))
      throw new Error('Invalid GitHub API endpoint');
    const pages = [], seen = new Set();
    let path = endpoint;
    for (;;) {
      signal?.throwIfAborted();
      if (seen.has(path)) throw new Error('Repeated GitHub pagination URL');
      seen.add(path);
      const key = (text ? 'text:' : 'json:') + path;
      const old = !query && cache.get(key);
      let page;
      if (immutable && old) page = old;
      else {
        const args = ['api', path, '--hostname', 'github.com', '--method', query ? 'POST' : 'GET', '--include'];
        if (query) args.push('-f', 'query=' + query);
        if (text) args.push('--allow-escape-sequences');
        if (old?.etag) args.push('-H', 'If-None-Match: ' + old.etag);
        let result, failure;
        try { result = await execute('gh', args, {
          shell: false, windowsHide: true, encoding: 'utf8', timeout: 30000,
          maxBuffer: 32 * 1024 * 1024, signal,
          env: { ...process.env, GH_PROMPT_DISABLED: '1', GH_NO_UPDATE_NOTIFIER: '1', GH_PAGER: 'cat' },
        }); } catch (error) { failure = error; result = error; }
        if (signal?.aborted) signal.throwIfAborted();
        let received;
        try { received = response(result.stdout); } catch (error) {
          if (!failure) throw error;
          const detail = String(failure.stderr || failure.message);
          throw Object.assign(new Error('gh query failed: ' + detail), {
            transient: Boolean(failure.killed || /timed? ?out|ECONNRESET|ETIMEDOUT|TLS handshake|connection reset|dial tcp/i.test(detail)),
          });
        }
        const { status, headers, body } = received;
        if (status === 304 && old) {
          page = { ...old, ...(headers.link !== undefined ? { next: nextPage(headers.link) } : {}) };
          cache.set(key, page);
        }
        else if (failure || status >= 400) {
          const rateLimit = status === 429 || (status === 403 &&
            (headers['x-ratelimit-remaining'] === '0' || headers['retry-after'] || /rate limit/i.test(body)));
          let retryAt = 0;
          if (headers['retry-after']) {
            const value = headers['retry-after'];
            retryAt = /^\d+$/.test(value) ? now() + Number(value) * 1000 : Date.parse(value);
          }
          if (headers['x-ratelimit-remaining'] === '0')
            retryAt = Math.max(retryAt || 0, Number(headers['x-ratelimit-reset']) * 1000 || 0);
          if (rateLimit && !retryAt) retryAt = now() + 60000;
          throw Object.assign(new Error('GitHub HTTP ' + status + ': ' + body.slice(0, 1000)), {
            transient: rateLimit || status >= 500, retryAt,
          });
        } else {
          let data;
          if (text) data = body;
          else { try { data = JSON.parse(body); } catch { throw new Error('Invalid gh JSON'); } }
          if (query && data?.errors?.length) throw new Error('GraphQL query failed: ' + JSON.stringify(data.errors));
          const next = nextPage(headers.link);
          page = { data, next, etag: headers.etag };
          if (!query) cache.set(key, page);
        }
      }
      if (!paginate) return page.data;
      const data = collection ? page.data?.[collection] : page.data;
      if (!Array.isArray(data)) throw new Error('Invalid paginated collection');
      pages.push(...data);
      if (!page.next) return pages;
      path = page.next;
    }
  };
}
const defaultApi = createGithubApi();
export function ghApi(endpoint, config = {}) {
  if (config.execute) return createGithubApi({ execute: config.execute, now: config.now })(endpoint, config);
  return defaultApi(endpoint, config);
}