const evidenceBody = document.querySelector('#evidence-body');
const poolsBody = document.querySelector('#pools-body');
const detail = document.querySelector('#detail');
let evidence = [];

function make(tag, content, className) {
  const element = document.createElement(tag);
  if (content !== undefined) element.textContent = content;
  if (className) element.className = className;
  return element;
}

function shortHash(value) {
  const normalized = hashHex(value);
  if (!normalized) return '未提交';
  return `${normalized.slice(0, 10)}…${normalized.slice(-8)}`;
}

function hashHex(value) {
  if (typeof value === 'string') return value.replace(/^0x/, '').toLowerCase();
  if (Array.isArray(value)) return value.map((byte) => Number(byte).toString(16).padStart(2, '0')).join('');
  return '';
}

function formatAmount(value) {
  if (value === undefined || value === null) return '—';
  return String(value);
}

function renderPools(pools) {
  poolsBody.replaceChildren();
  if (!pools.length) {
    const row = make('tr');
    const cell = make('td', '尚未保存池状态快照', 'empty');
    cell.colSpan = 5;
    row.append(cell);
    poolsBody.append(row);
    return;
  }
  for (const item of pools) {
    const snapshot = item.snapshot ?? item;
    const row = make('tr');
    const address = snapshot.pool_id?.Contract ?? snapshot.pool_id ?? item.pool_address;
    const assets = `${assetLabel(snapshot.asset_0 ?? item.token0)} → ${assetLabel(snapshot.asset_1 ?? item.token1)}`;
    const reserves = `${formatAmount(snapshot.reserve_0)} / ${formatAmount(snapshot.reserve_1)}`;
    const values = [shortHash(address), assets, reserves, `${snapshot.fee_bps ?? item.fee_bps ?? '—'} bps`, snapshot.observed_at?.number ?? item.observed_block ?? '—'];
    for (const value of values) row.append(make('td', String(value), 'mono'));
    poolsBody.append(row);
  }
}

function assetLabel(asset) {
  if (typeof asset === 'string') return asset;
  if (asset?.Native !== undefined) return 'Native';
  if (asset?.Contract) return shortHash(asset.Contract);
  return '未知资产';
}

function renderEvidence() {
  const query = document.querySelector('#search').value.trim().toLowerCase();
  const filtered = evidence.filter((item) => {
    const joined = [hashHex(item.execution?.intent_id), hashHex(item.route_hash), hashHex(item.execution?.tx_hash)].join(' ');
    return joined.includes(query);
  });
  evidenceBody.replaceChildren();
  if (!filtered.length) {
    const row = make('tr');
    const cell = make('td', evidence.length ? '没有匹配的证据' : '尚无执行证据', 'empty');
    cell.colSpan = 7;
    row.append(cell);
    evidenceBody.append(row);
    return;
  }
  for (const item of filtered) {
    const row = make('tr', undefined, 'selectable');
    const quote = item.quote;
    const tx = item.execution?.tx_hash;
    const receipt = item.receipt;
    const result = item.failure_reason ?? receipt?.failure_reason ?? (receipt?.success ? '执行成功' : '未提交');
    const values = [
      shortHash(item.execution?.intent_id),
      item.execution?.status ?? '未知',
      quote ? `${quote.amount_out} / ${quote.score}` : '—',
      shortHash(item.route_hash),
      shortHash(tx),
      receipt ? `${receipt.block?.number ?? '—'} / ${item.events?.length ?? 0}` : '—',
      result,
    ];
    for (const [index, value] of values.entries()) {
      const cell = make('td', String(value), index === 0 || index === 3 || index === 4 ? 'mono' : '');
      if (index === 1) {
        cell.replaceChildren(make('span', String(value), `status ${String(value).toLowerCase()}`));
      }
      row.append(cell);
    }
    row.addEventListener('click', () => renderDetail(item));
    evidenceBody.append(row);
  }
}

function renderDetail(item) {
  detail.hidden = false;
  detail.replaceChildren();
  const grid = make('div', undefined, 'detail-grid');
  const entries = [
    ['Intent', hashHex(item.execution?.intent_id)],
    ['输入指纹', hashHex(item.input_hash)],
    ['Route hash', hashHex(item.route_hash)],
    ['Commitment', hashHex(item.execution?.commitment_hash)],
    ['输入数量', item.intent?.amount_in ?? '—'],
    ['最小输出', item.intent?.amount_out_min ?? '—'],
    ['截止区块', item.intent?.deadline ?? '—'],
    ['交易 hash', hashHex(item.execution?.tx_hash) || '未提交'],
    ['区块 hash', hashHex(item.execution?.block_hash) || '未确认'],
    ['模拟耗时', item.simulation_duration_ms === null ? '—' : `${item.simulation_duration_ms} ms`],
    ['提交耗时', item.submission_duration_ms === null ? '—' : `${item.submission_duration_ms} ms`],
    ['失败原因', item.failure_reason ?? '无'],
  ];
  for (const [label, value] of entries) {
    const cell = make('div', undefined, 'detail-item');
    cell.append(make('span', label), make('strong', String(value ?? '—')));
    grid.append(cell);
  }
  detail.append(grid);
  if (item.route?.steps?.length) {
    const routes = make('ol', undefined, 'route-steps');
    for (const [index, step] of item.route.steps.entries()) {
      routes.append(make('li', `${index + 1}. ${shortHash(step.pool_id)} · ${assetLabel(step.asset_in)} → ${assetLabel(step.asset_out)}`));
    }
    detail.append(routes);
  }
  if (item.quote?.risk_fields) {
    detail.append(make('pre', JSON.stringify(item.quote.risk_fields, null, 2), 'risk-details'));
  }
  if (item.events?.length) {
    const events = make('ul', undefined, 'event-list');
    for (const event of item.events) {
      const payload = Array.isArray(event.data) ? event.data.map((byte) => Number(byte).toString(16).padStart(2, '0')).join('') : '';
      events.append(make('li', `${event.pallet}::${event.event} · #${event.event_index} · 0x${payload}`));
    }
    detail.append(events);
  }
}

function average(values) {
  return values.length ? `${Math.round(values.reduce((sum, value) => sum + value, 0) / values.length)} ms` : '--';
}

async function refresh() {
  document.querySelector('#refresh').disabled = true;
  try {
    const response = await fetch('/api/overview', { cache: 'no-store' });
    const snapshot = await response.json();
    evidence = snapshot.evidence ?? [];
    renderEvidence();
    renderPools(snapshot.pools ?? []);
    const confirmed = evidence.filter((item) => item.execution?.status === 'Confirmed').length;
    const finalized = evidence.filter((item) => item.receipt?.tx_hash).length;
    const dryRuns = evidence.map((item) => item.simulation_duration_ms).filter(Number.isFinite);
    const latestRun = snapshot.runs?.[0];
    document.querySelector('#database-state').textContent = snapshot.database === 'ready' ? '证据库已连接 · 只读' : snapshot.database === 'missing' ? '本地证据库尚未创建' : '证据库暂不可读取';
    document.querySelector('#metric-run').textContent = latestRun?.run_id ?? '未采集';
    document.querySelector('#metric-runtime').textContent = latestRun?.runtime_version ?? 'runtime 信息未采集';
    document.querySelector('#metric-pools').textContent = String(snapshot.pools?.length ?? 0);
    document.querySelector('#metric-evidence').textContent = String(evidence.length);
    document.querySelector('#metric-status').textContent = `${confirmed} 笔已确认交易`;
    document.querySelector('#metric-latency').textContent = average(dryRuns);
    document.querySelector('#snapshot-block').textContent = latestRun?.end_block ? `最新区块 ${latestRun.end_block}` : '尚无区块快照';
    document.querySelector('#requirement-transactions').textContent = `${finalized} / 10`;
    const deployments = snapshot.deployments ?? [];
    document.querySelector('#requirement-deployments').textContent = deployments.length ? `${deployments.length} 条` : '未采集';
    document.querySelector('#requirement-performance').textContent = dryRuns.length ? `${dryRuns.length} 个样本` : '未采集';
    document.querySelector('#updated-at').textContent = `更新于 ${new Date().toLocaleTimeString('zh-CN', { hour12: false })}`;
  } catch {
    document.querySelector('#database-state').textContent = '本地 API 暂不可用';
  } finally {
    document.querySelector('#refresh').disabled = false;
  }
}

document.querySelector('#refresh').addEventListener('click', refresh);
document.querySelector('#search').addEventListener('input', renderEvidence);
refresh();
