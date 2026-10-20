// Read-only protocol smoke check. Prints aggregates, never user content or tokens.
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { codexCommand } from './probe-discovery.mjs';

let launch;
try { launch = codexCommand(); }
catch (error) { console.error(error.message); process.exit(1); }
const child = spawn(launch.command, launch.args, { stdio: ['pipe', 'pipe', 'ignore'], shell: false, windowsHide: true });
const pending = new Map();
let next = 0;
const lines = createInterface({ input: child.stdout });
lines.on('line', line => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (!pending.has(msg.id)) return;
  const { resolve, reject, timer } = pending.get(msg.id);
  pending.delete(msg.id); clearTimeout(timer);
  if (msg.error) reject(new Error(`RPC ${msg.error.code}: request failed`));
  else resolve(msg.result);
});
child.on('error', () => { for (const p of pending.values()) { clearTimeout(p.timer); p.reject(new Error('Unable to start Codex. Check the official CLI installation.')); } pending.clear(); });
child.on('exit', () => { for (const p of pending.values()) { clearTimeout(p.timer); p.reject(new Error('Server exited')); } pending.clear(); });
function rpc(method, params = {}) {
  const id = ++next;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method}: timeout`)); }, 20000);
    pending.set(id, { resolve, reject, timer });
    child.stdin.write(JSON.stringify({ id, method, params }) + '\n');
  });
}
try {
  const init = await rpc('initialize', { clientInfo: { name: 'codex_autoresume', title: 'Codex AutoResume', version: '0.1.0' } });
  child.stdin.write(JSON.stringify({ method: 'initialized', params: {} }) + '\n');
  const account = await rpc('account/read', { refreshToken: false });
  const list = await rpc('thread/list', { limit: 10, sourceKinds: ['cli', 'vscode', 'appServer', 'exec'], useStateDbOnly: true, sortKey: 'updated_at' });
  const report = { handshake: Boolean(init.userAgent), accountType: account.account?.type ?? null, discoveredOnFirstPage: list.data.length, hasMore: Boolean(list.nextCursor), threadRead: 'no thread', quotas: 'not queried' };
  if (list.data.length) {
    const read = await rpc('thread/read', { threadId: list.data[0].id, includeTurns: false });
    report.threadRead = read.thread.id === list.data[0].id ? 'passed' : 'mismatch';
  }
  if (account.account?.type === 'chatgpt') {
    try {
      const limits = await rpc('account/rateLimits/read');
      report.quotas = 'passed'; report.bucketCount = Object.keys(limits.rateLimitsByLimitId ?? {}).length;
      report.hasServerReset = [limits.rateLimits, ...Object.values(limits.rateLimitsByLimitId ?? {})].some(b => b?.primary?.resetsAt || b?.secondary?.resetsAt);
    } catch { report.quotas = 'unavailable'; }
  }
  console.log(JSON.stringify(report, null, 2));
} catch (error) { console.error(error.message); process.exitCode = 1; }
finally { lines.close(); child.stdin.end(); child.kill(); }
