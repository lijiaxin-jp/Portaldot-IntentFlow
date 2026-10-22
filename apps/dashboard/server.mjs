import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { DatabaseSync } from 'node:sqlite';
import { dirname, extname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const publicRoot = resolve(root, 'apps/dashboard/public');
const databasePath = resolve(root, process.env.INTENTFLOW_EVIDENCE_DB ?? 'data/intentflow.sqlite3');
const host = '127.0.0.1';
const port = Number(process.env.PORT ?? '8787');
const contentTypes = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.css', 'text/css; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
]);

function readEvidence(limit = 250) {
  if (!existsSync(databasePath)) {
    return { database: 'missing', evidence: [], runs: [], pools: [] };
  }
  let database;
  try {
    database = new DatabaseSync(databasePath, { readOnly: true });
    database.exec('PRAGMA query_only = ON');
    const evidence = database.prepare(
      'SELECT payload_json FROM portaldot_evidence ORDER BY created_at_ms DESC LIMIT ?',
    ).all(limit).map((row) => JSON.parse(row.payload_json));
    const runs = database.prepare(
      'SELECT payload_json FROM portaldot_runs ORDER BY created_at_ms DESC LIMIT 20',
    ).all().map((row) => JSON.parse(row.payload_json));
    const pools = database.prepare(
      'SELECT payload_json FROM portaldot_pools ORDER BY observed_block DESC LIMIT 100',
    ).all().map((row) => JSON.parse(row.payload_json));
    const deployments = database.prepare(
      'SELECT payload_json FROM portaldot_deployments ORDER BY contract_kind',
    ).all().map((row) => JSON.parse(row.payload_json));
    return { database: 'ready', evidence, runs, pools, deployments };
  } catch (error) {
    return { database: 'unavailable', evidence: [], runs: [], pools: [], deployments: [] };
  } finally {
    database?.close();
  }
}

function sendJson(response, status, value) {
  response.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    'x-content-type-options': 'nosniff',
  });
  response.end(JSON.stringify(value));
}

async function handle(request, response) {
  const url = new URL(request.url ?? '/', `http://${host}:${port}`);
  if (request.method !== 'GET') {
    sendJson(response, 405, { error: 'method not allowed' });
    return;
  }
  if (url.pathname === '/api/health') {
    const snapshot = readEvidence(1);
    sendJson(response, 200, { status: snapshot.database, readOnly: true });
    return;
  }
  if (url.pathname === '/api/overview' || url.pathname === '/api/evidence') {
    const limit = Math.max(1, Math.min(500, Number(url.searchParams.get('limit') ?? '250')));
    const snapshot = readEvidence(limit);
    sendJson(response, 200, snapshot);
    return;
  }
  const evidenceMatch = url.pathname.match(/^\/api\/evidence\/([0-9a-fA-F]{64})$/);
  if (evidenceMatch) {
    const snapshot = readEvidence(500);
    const id = evidenceMatch[1].toLowerCase();
    const record = snapshot.evidence.find((entry) => hashHex(entry.execution?.intent_id) === id);
    sendJson(response, record ? 200 : 404, record ?? { error: 'evidence not found' });
    return;
  }
  const relative = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
  if (!['index.html', 'styles.css', 'app.js'].includes(relative)) {
    sendJson(response, 404, { error: 'not found' });
    return;
  }
  try {
    const body = await readFile(resolve(publicRoot, relative));
    response.writeHead(200, {
      'content-type': contentTypes.get(extname(relative)) ?? 'application/octet-stream',
      'x-content-type-options': 'nosniff',
      'content-security-policy': "default-src 'self'; connect-src 'self'; style-src 'self'; script-src 'self'; img-src 'self' data:",
    });
    response.end(body);
  } catch {
    sendJson(response, 404, { error: 'not found' });
  }
}

function hashHex(value) {
  if (typeof value === 'string') return value.replace(/^0x/, '').toLowerCase();
  if (Array.isArray(value)) return value.map((byte) => Number(byte).toString(16).padStart(2, '0')).join('');
  return '';
}

if (!Number.isInteger(port) || port < 1 || port > 65535) {
  throw new Error('PORT must be an integer between 1 and 65535');
}

createServer((request, response) => {
  handle(request, response).catch(() => sendJson(response, 500, { error: 'internal server error' }));
}).listen(port, host, () => {
  process.stdout.write(`IntentFlow dashboard: http://${host}:${port}\n`);
});
