const requestedRole = new URLSearchParams(window.location.search).get('role');
const role = requestedRole === 'widget' ? 'widget' : 'main';
document.body.dataset.role = role;
document.body.dataset.expanded = 'false';

const invoke = (command, args = {}) => {
  if (window.__TAURI_INTERNALS__?.invoke) return window.__TAURI_INTERNALS__.invoke(command, args);
  const api = window.__TAURI__;
  const fn = api?.core?.invoke || api?.invoke;
  return typeof fn === 'function'
    ? fn(command, args)
    : Promise.reject(new Error('Tauri IPC 未注入'));
};

const el = id => document.getElementById(id);
const esc = value => String(value ?? '').replace(/[&<>"']/g, character => ({
  '&': '&amp;',
  '<': '&lt;',
  '>': '&gt;',
  '"': '&quot;',
  "'": '&#39;',
})[character]);
const icon = name => '<svg class="glyph" aria-hidden="true"><use href="#icon-' + esc(name) + '"/></svg>';
const clamp = (value, minimum, maximum) => Math.min(maximum, Math.max(minimum, value));
const sum = values => values.reduce((total, value) => total + Number(value || 0), 0);

const statusLabels = {
  ready: '已同步',
  syncing: '同步中',
  connecting: '连接中',
  stale: '数据陈旧',
  offline: '离线',
  auth_required: '需要认证',
  rate_limited: '已限流',
  provider_error: 'Provider 错误',
  disabled: '已停用',
};

const statusClasses = {
  ready: 'ready',
  syncing: 'neutral',
  connecting: 'neutral',
  stale: 'error',
  rate_limited: 'error',
  offline: 'error',
  auth_required: 'error',
  provider_error: 'error',
  disabled: 'error',
};

const authLabels = {
  oauth_browser: '浏览器登录',
  oauth_device_code: '设备码登录',
  admin_api_key: 'Admin API Key',
  api_key: 'API Key',
};

const defaultTrayPreferences = {
  account: true,
  quota: true,
  todayTokens: false,
  todayCost: false,
  codexSessions: false,
  trend: true,
};

function readTrayPreferences() {
  try {
    const saved = JSON.parse(localStorage.getItem('llm-meter-tray-preferences') || '{}');
    return Object.assign({}, defaultTrayPreferences, saved);
  } catch (_) {
    return Object.assign({}, defaultTrayPreferences);
  }
}

const state = {
  snapshot: { connections: [], local_codex: {} },
  providers: [],
  activePage: 'overview',
  selectedConnectionId: '',
  widgetExpanded: false,
  hasLoaded: false,
  loading: true,
  providerLoading: role !== 'widget',
  providerCatalogLoaded: false,
  loadError: '',
  busyAction: '',
  actionMessage: '',
  pendingRemoveId: '',
  modal: null,
  autostart: {
    known: false,
    loading: false,
    enabled: false,
    active: false,
    installed: false,
    state: 'unknown',
  },
  trayPreferences: readTrayPreferences(),
};

let toastTimer = null;
let removeTimer = null;
let lastTrayTooltip = '';

function connections() {
  return Array.isArray(state.snapshot.connections) ? state.snapshot.connections : [];
}

function localCodex() {
  return state.snapshot.local_codex || {};
}

function selectedConnection() {
  return connections().find(connection => connection.id === state.selectedConnectionId)
    || connections()[0]
    || null;
}

function normalizeSelection() {
  if (!connections().some(connection => connection.id === state.selectedConnectionId)) {
    state.selectedConnectionId = connections()[0]?.id || '';
  }
}

function compact(value) {
  const number = Number(value || 0);
  if (Math.abs(number) >= 1000000) return (number / 1000000).toFixed(2) + 'M';
  if (Math.abs(number) >= 1000) return (number / 1000).toFixed(1) + 'K';
  return Math.round(number).toLocaleString('zh-CN');
}

function compactTray(value) {
  const number = Number(value || 0);
  if (Math.abs(number) >= 1000000000) return (number / 1000000000).toFixed(2) + 'B';
  return compact(number);
}

function currency(value) {
  const amount = Number(value || 0);
  return '$' + amount.toFixed(amount > 0 && amount < 1 ? 4 : 2);
}

function currencyFixed2(value) {
  return '$' + Number(value || 0).toFixed(2);
}

function localDateTime(value) {
  if (!value) return '永久有效';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '未知';
  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).replace(/\//g, '-');
}

function relativeTime(value) {
  if (!value) return '尚未同步';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '尚未同步';
  const seconds = Math.max(0, Math.round((Date.now() - date.getTime()) / 1000));
  if (seconds < 45) return '刚刚';
  if (seconds < 3600) return Math.floor(seconds / 60) + ' 分钟前';
  if (seconds < 86400) return Math.floor(seconds / 3600) + ' 小时前';
  if (seconds < 604800) return Math.floor(seconds / 86400) + ' 天前';
  return date.toLocaleDateString('zh-CN');
}

function localDateKey(date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return year + '-' + month + '-' + day;
}

function periodContext() {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const keys = [];
  const dates = [];
  for (let index = 6; index >= 0; index -= 1) {
    const date = new Date(today);
    date.setDate(date.getDate() - index);
    dates.push(date);
    keys.push(localDateKey(date));
  }
  return { now, today, keys, dates };
}

function statusText(status) {
  return statusLabels[status] || status || '未知';
}

function providerIcon(providerId) {
  if (providerId === 'openai') return 'terminal';
  if (providerId === 'kimi') return 'message-circle';
  return 'server';
}

function providerMark(providerId) {
  return '<span class="provider-mark">' + icon(providerIcon(providerId)) + '</span>';
}

function isRelayConnection(connection) {
  return Boolean(connection && connection.provider_id === 'relay');
}

function isLocallyTrackedSubscription(connection) {
  return Boolean(connection && connection.connection_type === 'chatgpt_subscription');
}

function connectionQuota(connection) {
  const quotas = Array.isArray(connection?.quota_windows) ? connection.quota_windows : [];
  let best = null;
  for (const quota of quotas) {
    if (quota.remaining_ratio == null) continue;
    if (!best || Number(quota.remaining_ratio) < Number(best.remaining_ratio)) best = quota;
  }
  return best;
}

function connectionQuotaPercent(connection) {
  const quota = connectionQuota(connection);
  return quota ? clamp(Math.round(Number(quota.remaining_ratio) * 100), 0, 100) : -1;
}

function connectionQuotaName(connection) {
  const quota = connectionQuota(connection);
  return quota ? (quota.display_name || '当前额度') : '额度未提供';
}

function connectionQuotaResetsAt(connection) {
  const quota = connectionQuota(connection);
  return quota?.resets_at ? localDateTime(quota.resets_at) : '—';
}

function connectionExhaustionForecast(connection) {
  if (!connection || isRelayConnection(connection)) return '';
  const quota = connectionQuota(connection);
  if (!quota) return '正在积累用量样本';
  const now = new Date();
  const start = quota.window_start ? new Date(quota.window_start) : null;
  const reset = quota.resets_at
    ? new Date(quota.resets_at)
    : (quota.window_end ? new Date(quota.window_end) : null);
  const remaining = quota.remaining_ratio == null ? null : Number(quota.remaining_ratio);
  const used = remaining == null ? null : 1 - remaining;
  if (start && !Number.isNaN(start.getTime()) && used != null && used > 0 && now > start) {
    const exhaustsAt = new Date(start.getTime() + (now.getTime() - start.getTime()) / used);
    if (!reset || Number.isNaN(reset.getTime()) || exhaustsAt < reset) {
      return '预计耗尽 ' + localDateTime(exhaustsAt);
    }
    return '本周期预计不会耗尽';
  }
  const forecast = localCodex().weekly_quota_forecast;
  if (connection.connection_type === 'chatgpt_subscription' && forecast) {
    return forecast.exhausts_at
      ? '预计耗尽 ' + localDateTime(forecast.exhausts_at)
      : '本周期预计不会耗尽';
  }
  return '正在积累用量样本';
}

function metricUnitKey(unit) {
  if (!unit) return 'credit';
  if (typeof unit === 'string') return unit;
  return unit.kind === 'currency'
    ? String(unit.code || 'USD').toUpperCase()
    : String(unit.kind || 'credit');
}

function formatSpent(value, unit) {
  const amount = Number(value || 0);
  const digits = amount > 0 && amount < 1 ? 4 : 2;
  if (unit === 'USD') return '$' + amount.toFixed(digits);
  if (unit === 'credit') return amount.toFixed(digits) + ' Credits';
  return amount.toFixed(digits) + ' ' + unit;
}

function isCurrentMetric(metric, now) {
  if (!metric?.period_start) return false;
  const start = new Date(metric.period_start);
  const end = metric.period_end ? new Date(metric.period_end) : null;
  return !Number.isNaN(start.getTime())
    && start <= now
    && (!end || Number.isNaN(end.getTime()) || now < end);
}

function accountMetrics(connection) {
  return (Array.isArray(connection?.metrics) ? connection.metrics : [])
    .filter(metric => metric.scope === 'account' || !metric.dimensions || !metric.dimensions.model);
}

function connectionTodaySpent(connection) {
  const now = new Date();
  const metrics = accountMetrics(connection).filter(metric => isCurrentMetric(metric, now));
  let selectedMetrics = metrics.filter(metric => metric.metric_key === 'cost.actual');
  if (!selectedMetrics.length) selectedMetrics = metrics.filter(metric => metric.metric_key === 'credit.used');
  if (!selectedMetrics.length) selectedMetrics = metrics.filter(metric => metric.metric_key === 'cost.estimated');
  if (!selectedMetrics.length) return '今日暂无消费记录';
  const totals = {};
  for (const metric of selectedMetrics) {
    const unit = metricUnitKey(metric.unit);
    totals[unit] = Number(totals[unit] || 0) + Number(metric.value || 0);
  }
  return Object.keys(totals).map(unit => formatSpent(totals[unit], unit)).join(' + ');
}

function connectionTodayTokenValue(connection, key) {
  const now = new Date();
  return accountMetrics(connection)
    .filter(metric => metric.metric_key === key && isCurrentMetric(metric, now))
    .reduce((total, metric) => total + Number(metric.value || 0), 0);
}

function connectionTodayTokens(connection) {
  const input = connectionTodayTokenValue(connection, 'token.input');
  const cached = connectionTodayTokenValue(connection, 'token.cached_input');
  const output = connectionTodayTokenValue(connection, 'token.output');
  if (!input && !cached && !output) return '今日暂无 Token 记录';
  return '输入 ' + compact(input) + ' · 缓存 ' + compact(cached) + ' · 输出 ' + compact(output);
}

function localSubscriptionTodayCost() {
  const value = localCodex().today_estimated_cost_usd;
  if (value == null) return '本地费用样本不足';
  return '官方标准价实时估算 ' + currency(value);
}

function metricDateKey(metric, now) {
  if (isCurrentMetric(metric, now)) return localDateKey(now);
  const start = new Date(metric.period_start);
  return Number.isNaN(start.getTime()) ? '' : localDateKey(start);
}

function connectionPeriodValues(connection, keys, now) {
  const values = [0, 0, 0, 0, 0, 0, 0];
  if (!connection) return values;
  if (isLocallyTrackedSubscription(connection)) {
    for (const usage of localCodex().daily_usage || []) {
      const index = keys.indexOf(String(usage.date || ''));
      if (index >= 0) values[index] = Number(usage.total_tokens || 0);
    }
    return values;
  }
  const metrics = accountMetrics(connection);
  const totals = metrics.filter(metric => metric.metric_key === 'token.total' && metric.period_start);
  const source = totals.length
    ? totals
    : metrics.filter(metric => (metric.metric_key === 'token.input' || metric.metric_key === 'token.output') && metric.period_start);
  for (const metric of source) {
    const index = keys.indexOf(metricDateKey(metric, now));
    if (index >= 0) values[index] += Number(metric.value || 0);
  }
  return values;
}

function selectedOverviewValues(connection) {
  if (!connection) {
    return {
      tokens: '—',
      cost: '—',
      values: [0, 0, 0, 0, 0, 0, 0],
      total: '—',
    };
  }
  const context = periodContext();
  const values = connectionPeriodValues(connection, context.keys, context.now);
  let cost = '—';
  if (isLocallyTrackedSubscription(connection)) {
    if (localCodex().today_estimated_cost_usd != null) cost = currency(localCodex().today_estimated_cost_usd);
  } else {
    let costs = accountMetrics(connection)
      .filter(metric => metric.metric_key === 'cost.actual' && isCurrentMetric(metric, context.now));
    if (!costs.length) {
      costs = accountMetrics(connection)
        .filter(metric => metric.metric_key === 'cost.estimated' && isCurrentMetric(metric, context.now));
    }
    if (costs.length) cost = currency(sum(costs.map(metric => metric.value)));
  }
  return {
    tokens: compact(values[6]),
    cost,
    values,
    total: compact(sum(values)) + ' Token',
  };
}

function resetSummaries() {
  return connections()
    .map(connection => connection.rate_limit_reset_credits)
    .filter(summary => summary != null);
}

function resetCreditItems() {
  return resetSummaries().flatMap(summary => Array.isArray(summary.credits) ? summary.credits : []);
}

function resetAvailableCount() {
  return resetSummaries().reduce((total, summary) => total + Number(summary.available_count || 0), 0);
}

function resetDetailsKnown() {
  return resetSummaries().some(summary => Array.isArray(summary.credits));
}

function nearestResetCredit() {
  const now = Date.now();
  const available = resetCreditItems()
    .filter(credit => credit?.status === 'available'
      && credit.expires_at
      && new Date(credit.expires_at).getTime() > now)
    .sort((left, right) => new Date(left.expires_at).getTime() - new Date(right.expires_at).getTime());
  return available[0] || null;
}

function resetStatusText(status) {
  const labels = {
    available: '可使用',
    redeeming: '使用中',
    redeemed: '已使用',
    unknown: '未知',
  };
  return labels[status] || status || '未知';
}

function hasSubscription() {
  return connections().some(connection => connection.connection_type === 'chatgpt_subscription');
}

function statusMarkup(status) {
  return '<span class="status-text ' + (statusClasses[status] || 'neutral') + '">'
    + esc(statusText(status)) + '</span>';
}

function gaugeMarkup(connection) {
  const percent = connectionQuotaPercent(connection);
  const value = percent < 0 ? 0 : percent;
  return '<progress class="gauge' + (percent >= 0 && percent < 20 ? ' critical' : '') + '" max="100" value="'
    + value + '" aria-label="' + (percent < 0 ? '额度未提供' : percent + '% 剩余') + '"></progress>';
}

function quotaFooterMarkup(connection) {
  if (isRelayConnection(connection)) return '';
  return '<div class="quota-footer">'
    + icon('calendar-clock')
    + '<span class="reset-at">下次重置 ' + esc(connectionQuotaResetsAt(connection)) + '</span>'
    + '<span class="forecast" title="' + esc(connectionExhaustionForecast(connection)) + '">'
    + esc(connectionExhaustionForecast(connection)) + '</span>'
    + '</div>';
}

function accountQuotaFooterMarkup(connection) {
  if (isRelayConnection(connection)) return '';
  return '<div class="quota-footer account-quota-footer">'
    + '<span class="reset-at">下次重置 ' + esc(connectionQuotaResetsAt(connection)) + '</span>'
    + '<span class="forecast" title="' + esc(connectionExhaustionForecast(connection)) + '">'
    + esc(connectionExhaustionForecast(connection)) + '</span>'
    + '</div>';
}

function accountOptionMarkup(connection) {
  const selected = state.selectedConnectionId === connection.id;
  const relay = isRelayConnection(connection);
  const percent = connectionQuotaPercent(connection);
  const metricLabel = relay ? '今日已消费' : connectionQuotaName(connection);
  const metricValue = relay
    ? connectionTodaySpent(connection)
    : (percent >= 0 ? percent + '% 剩余' : '额度未提供');
  let detail = '';
  if (isLocallyTrackedSubscription(connection)) {
    detail = '<div class="account-meta">' + esc(localSubscriptionTodayCost()) + '</div>';
  } else if (relay) {
    detail = '<div class="account-meta">' + esc(connectionTodayTokens(connection)) + '</div>';
  }
  return '<li><button type="button" class="account-option' + (selected ? ' selected' : '')
    + '" data-select-account="' + esc(connection.id) + '" title="' + esc(connection.display_name || 'Provider') + '">'
    + providerMark(connection.provider_id)
    + '<span class="account-copy">'
    + '<span class="account-topline"><strong class="account-name">' + esc(connection.display_name || 'OpenAI')
    + '</strong>' + statusMarkup(connection.status) + '</span>'
    + '<span class="metric-line"><span>' + esc(metricLabel) + '</span><strong>' + esc(metricValue) + '</strong></span>'
    + detail
    + (relay ? '' : gaugeMarkup(connection))
    + quotaFooterMarkup(connection)
    + '</span></button></li>';
}

function trendCardMarkup(connection, values) {
  const maximum = Math.max.apply(null, values.concat([1]));
  const left = 2;
  const right = 558;
  const top = 6;
  const baseline = 112;
  const points = values.map((value, index) => ({
    x: left + (right - left) * index / Math.max(values.length - 1, 1),
    y: baseline - (baseline - top) * Number(value || 0) / maximum,
  }));
  const pointText = points.map(point => point.x.toFixed(2) + ',' + point.y.toFixed(2)).join(' ');
  const area = 'M ' + points[0].x.toFixed(2) + ' ' + baseline
    + ' L ' + points.map(point => point.x.toFixed(2) + ' ' + point.y.toFixed(2)).join(' L ')
    + ' L ' + points[points.length - 1].x.toFixed(2) + ' ' + baseline + ' Z';
  const chart = '<div class="trend-wrap"><svg class="trend-svg" viewBox="0 0 560 118" preserveAspectRatio="none"'
    + ' role="img" aria-label="最近七天 Token 用量"><path class="trend-area" d="' + area + '"/>'
    + '<polyline class="trend-line" points="' + pointText + '"/></svg></div>';
  return '<section class="surface-card chart-card">'
    + '<div class="chart-head"><span class="chart-title">近 7 天 Token 趋势 · '
    + esc(connection?.display_name || '未选择账号') + '</span>'
    + '<strong class="chart-total">' + esc(compact(sum(values)) + ' Token') + '</strong></div>'
    + chart + '</section>';
}

function overviewQuotaMarkup(connection) {
  if (!connection || isRelayConnection(connection)) return '';
  const percent = connectionQuotaPercent(connection);
  const label = connectionQuotaName(connection);
  const value = percent >= 0 ? percent + '% 剩余' : '额度未提供';
  return '<section class="surface-card overview-quota">'
    + '<div class="metric-line"><span>' + esc(label) + '</span><strong>' + esc(value) + '</strong></div>'
    + gaugeMarkup(connection)
    + quotaFooterMarkup(connection)
    + '</section>';
}

function overviewPage() {
  const connection = selectedConnection();
  const overview = selectedOverviewValues(connection);
  const nearest = nearestResetCredit();
  const nearestUrgent = nearest
    ? new Date(nearest.expires_at).getTime() - Date.now() < 24 * 60 * 60 * 1000
    : false;
  const reset = nearest
    ? '<section class="surface-card reset-highlight' + (nearestUrgent ? ' urgent' : '') + '">'
      + '<span class="item-mark">' + icon('restore') + '</span>'
      + '<span class="item-copy"><span class="quiet-label">最近到期的 Reset 机会</span>'
      + '<strong class="reset-date">' + esc(localDateTime(nearest.expires_at)) + '</strong></span>'
      + '<span class="section-kicker accent-value">' + resetAvailableCount() + ' 次可用</span></section>'
    : '';
  return '<div class="page-stack">'
    + overviewQuotaMarkup(connection)
    + reset
    + '<div class="stats-grid">'
    + '<section class="surface-card stat-card"><span class="stat-label">' + icon('coins')
    + '<span>今日 Token</span></span><strong class="stat-value">' + esc(overview.tokens) + '</strong></section>'
    + '<section class="surface-card stat-card"><span class="stat-label">' + icon('dollar')
    + '<span>今日费用</span></span><strong class="stat-value">' + esc(overview.cost) + '</strong></section>'
    + '</div>'
    + trendCardMarkup(connection, overview.values)
    + '</div>';
}

function localCodexPage() {
  const local = localCodex();
  const sessions = Array.isArray(local.active_sessions) ? local.active_sessions : [];
  const models = Array.isArray(local.models) ? local.models : [];
  const summaryCost = local.estimated_cost_usd == null ? '—' : currencyFixed2(local.estimated_cost_usd);
  const modelMarkup = models.length
    ? '<div class="model-list">' + models.map(model => '<section class="surface-card model-card">'
      + '<div class="row-between"><strong class="item-title" title="' + esc(model.model || 'unknown') + '">'
      + esc(model.model || 'unknown') + '</strong><span class="model-cost">'
      + esc(model.estimated_cost_usd == null ? '价格未知' : currencyFixed2(model.estimated_cost_usd)) + '</span></div>'
      + '<div class="metric-detail">输入 ' + esc(compact(model.input_tokens)) + ' · 缓存 '
      + esc(compact(model.cached_input_tokens)) + ' · 输出 ' + esc(compact(model.output_tokens)) + '</div>'
      + '</section>').join('') + '</div>'
    : '';
  const sessionMarkup = sessions.length
    ? '<div class="session-list">' + sessions.map(session => '<section class="surface-card session-card">'
      + '<div class="row-between"><strong class="item-title">' + esc(session.model || 'unknown')
      + ' · ' + esc(compact(session.total_tokens)) + ' Token</strong></div>'
      + '<span class="session-path" title="' + esc(session.cwd || session.id || '') + '">'
      + esc(session.cwd || session.id || '本地会话') + '</span></section>').join('') + '</div>'
    : '';
  return '<div class="page-stack">'
    + '<section class="surface-card summary-card"><div class="summary-line">'
    + '<span class="item-mark">' + icon('terminal') + '</span>'
    + '<strong class="summary-title">' + sessions.length + ' 个运行中会话</strong>'
    + '<span class="summary-number">' + esc(summaryCost) + '</span></div>'
    + '<p class="summary-note">当前运行会话累计的 API 等价费用估算；ChatGPT 订阅不会按此金额额外收费</p></section>'
    + modelMarkup
    + sessionMarkup
    + '<div class="page-footnote">官方标准处理价格 · 更新于 ' + esc(local.pricing_as_of || '未知') + '</div>'
    + '</div>';
}

function resetsPage() {
  const summaries = resetSummaries();
  const items = resetCreditItems();
  const available = resetAvailableCount();
  const detailsKnown = resetDetailsKnown();
  const description = summaries.length
    ? (detailsKnown ? '以下时间均为本地时间' : 'Provider 仅返回总次数，暂未提供逐条过期时间')
    : '尚未获取 Reset 额度，请刷新订阅连接';
  let credits = '';
  if (items.length) {
    credits = '<div class="credit-list">' + items.map(credit => '<section class="surface-card credit-card">'
      + '<div class="reset-line"><strong class="item-title">' + esc(credit.title || 'Full reset') + '</strong>'
      + '<span class="credit-status' + (credit.status === 'available' ? ' available' : '') + '">'
      + esc(resetStatusText(credit.status)) + '</span></div>'
      + '<div class="reset-line"><span class="quiet-label">' + icon('calendar-clock') + ' 过期</span>'
      + '<span class="credit-expiry">' + esc(localDateTime(credit.expires_at)) + '</span></div>'
      + '</section>').join('') + '</div>';
  }
  const partial = summaries.length && detailsKnown && items.length < available
    ? '<section class="surface-card partial-details">服务端返回了 ' + available
      + ' 次机会，但仅提供 ' + items.length + ' 条明细。</section>'
    : '';
  return '<div class="page-stack">'
    + '<section class="surface-card summary-card reset-summary"><div class="summary-line">'
    + '<span class="item-mark">' + icon('restore') + '</span>'
    + '<strong class="summary-title">剩余 Reset 机会</strong>'
    + '<span class="summary-number">' + esc(summaries.length ? available + ' 次' : '—') + '</span></div>'
    + '<p class="summary-note">' + esc(description) + '</p></section>'
    + credits + partial + '</div>';
}

function budgetAmount(connection) {
  return connection?.budget?.amount == null ? '' : String(connection.budget.amount);
}

function connectionCardMarkup(connection) {
  const relay = isRelayConnection(connection);
  const selected = state.selectedConnectionId === connection.id;
  const percent = connectionQuotaPercent(connection);
  const removing = state.pendingRemoveId === connection.id;
  const budgetAllowed = !['chatgpt_subscription', 'kimi_code_subscription'].includes(connection.connection_type);
  let observed = '';
  if (isLocallyTrackedSubscription(connection)) {
    observed = '<div class="account-meta">' + esc(localSubscriptionTodayCost()) + '</div>';
  } else if (relay) {
    observed = '<div class="account-meta">' + esc(connectionTodayTokens(connection)) + '</div>';
  }
  const disabled = state.busyAction ? ' disabled' : '';
  const footer = (connection.connection_type === 'openai_compatible_proxy' || budgetAllowed)
    ? '<div class="connection-footer">'
      + (connection.connection_type === 'openai_compatible_proxy'
        ? '<button type="button" class="button" data-action="open-proxy" data-connection-id="' + esc(connection.id)
          + '"' + disabled + '>' + icon('server') + '<span>配置本地代理</span></button>'
        : '')
      + (budgetAllowed
        ? '<form class="budget-form" data-budget-form="' + esc(connection.id) + '">'
          + '<input name="amount" type="number" min="0.01" step="0.01" value="' + esc(budgetAmount(connection))
          + '" placeholder="月度预算 USD" aria-label="' + esc((connection.display_name || 'Provider') + ' 月度预算 USD')
          + '"' + disabled + ' required>'
          + '<button type="submit" class="button"' + (state.busyAction || !budgetAmount(connection) ? ' disabled' : '')
          + '><span>保存预算</span></button></form>'
        : '')
      + '</div>'
    : '';
  return '<article class="surface-card connection-card' + (selected ? ' selected' : '')
    + '" data-select-card="' + esc(connection.id) + '"><div class="connection-main">'
    + '<div class="connection-head"><div class="connection-identity">'
    + providerMark(connection.provider_id)
    + '<span class="connection-copy"><strong class="account-name" title="' + esc(connection.display_name || 'Provider') + '">'
    + esc(connection.display_name || 'Provider') + '</strong><span class="connection-status '
    + (statusClasses[connection.status] || 'neutral') + '">'
    + esc(connection.provider_id) + ' · ' + esc(statusText(connection.status)) + '</span></span></div>'
    + '<div class="connection-actions">'
    + '<button type="button" class="icon-button" data-action="refresh-connection" data-connection-id="' + esc(connection.id)
    + '" title="刷新连接" aria-label="刷新 ' + esc(connection.display_name || 'Provider') + '"' + disabled + '>' + icon('refresh') + '</button>'
    + '<button type="button" class="icon-button ' + (removing ? 'danger' : 'primary')
    + '" data-action="remove-connection" data-connection-id="'
    + esc(connection.id) + '" title="' + (removing ? '再次点击确认删除' : '删除连接') + '" aria-label="'
    + (removing ? '再次点击确认删除' : '删除 ' + esc(connection.display_name || 'Provider')) + '"' + disabled + '>'
    + icon(removing ? 'check' : 'trash') + '</button></div></div>'
    + '<span class="connection-type">' + esc(connection.connection_type) + '</span>'
    + '<div class="connection-data"><div class="metric-line"><span>'
    + esc(relay ? '今日已消费' : connectionQuotaName(connection)) + '</span><strong>'
    + esc(relay ? connectionTodaySpent(connection) : (percent >= 0 ? percent + '% 剩余' : '额度未提供'))
    + '</strong></div>' + observed + (relay ? '' : accountQuotaFooterMarkup(connection) + gaugeMarkup(connection))
    + '</div></div>' + footer + '</article>';
}

function accountsPage() {
  const cards = connections().length
    ? '<div class="connection-stack">' + connections().map(connectionCardMarkup).join('') + '</div>'
    : '<p class="accounts-empty">尚未添加账户。支持 ChatGPT、Kimi、OpenAI Platform 与任意 OpenAI 兼容 Provider。</p>';
  const message = state.actionMessage
    ? '<div class="action-message' + (state.actionMessage.includes('失败') ? ' error' : '') + '">'
      + esc(state.actionMessage) + '</div>'
    : '';
  return '<div class="page-stack">'
    + '<section class="surface-card accounts-intro"><div class="section-heading"><div>'
    + '<h2>账户与 Provider</h2><p>管理订阅、Platform API 和 OpenAI 兼容中转站</p></div>'
    + '<button type="button" class="button primary" data-action="open-add"' + (state.busyAction ? ' disabled' : '')
    + '>' + icon('plus') + '<span>添加</span></button>'
    + '</div></section>' + cards + message + '</div>';
}

function switchMarkup(key, checked, disabled, type) {
  const attribute = type === 'autostart'
    ? ' data-autostart-toggle'
    : ' data-tray-key="' + esc(key) + '"';
  return '<label class="switch"><input type="checkbox"' + attribute
    + (checked ? ' checked' : '') + (disabled ? ' disabled' : '')
    + ' aria-label="切换设置"><span class="switch-track"></span></label>';
}

function settingRowMarkup(label, description, control) {
  return '<div class="setting-row"><span class="setting-copy"><strong>' + esc(label)
    + '</strong><span>' + esc(description) + '</span></span>' + control + '</div>';
}

function autostartDescription() {
  if (state.autostart.loading) return '正在读取 Windows 任务计划状态';
  if (!state.autostart.known) return '暂时无法读取 Windows 任务计划状态';
  if (state.autostart.active) return 'daemon 已随当前用户会话运行';
  if (state.autostart.enabled) return '已注册，将在下次登录 Windows 时启动';
  return '登录 Windows 后自动启动 daemon';
}

function settingsPage() {
  const preferences = state.trayPreferences;
  const message = state.actionMessage
    ? '<div class="action-message' + (state.actionMessage.includes('失败') ? ' error' : '') + '">'
      + esc(state.actionMessage) + '</div>'
    : '';
  return '<div class="page-stack">'
    + '<section class="surface-card settings-card"><h2>应用</h2>'
    + settingRowMarkup('开机自动启动', autostartDescription(),
      switchMarkup('autostart', state.autostart.enabled, !state.autostart.known || state.autostart.loading || Boolean(state.busyAction), 'autostart'))
    + '<button type="button" class="button full settings-update" data-action="open-release">'
    + icon('download') + '<span>更新到最新稳定版</span></button>'
    + '</section>'
    + '<section class="surface-card settings-card"><h2>迷你组件显示</h2>'
    + '<p class="settings-description">选择 Windows 迷你组件中需要显示的信息，修改后立即生效。多个账号时，组件会自动显示剩余额度最低的账号。</p>'
    + settingRowMarkup('账号', '显示主账号名称（多个账号时显示剩余额度最低的账号）', switchMarkup('account', preferences.account, false, 'tray'))
    + settingRowMarkup('周额度', '显示所有账号中最低的剩余额度百分比', switchMarkup('quota', preferences.quota, false, 'tray'))
    + settingRowMarkup('今日 Token', '显示本地 Codex 今日 Token', switchMarkup('todayTokens', preferences.todayTokens, false, 'tray'))
    + settingRowMarkup('今日费用', '显示今日 API 等价费用', switchMarkup('todayCost', preferences.todayCost, false, 'tray'))
    + settingRowMarkup('Codex 会话数', '显示当前运行中的 Codex 数量', switchMarkup('codexSessions', preferences.codexSessions, false, 'tray'))
    + settingRowMarkup('Token 趋势', '显示最近七天的紧凑趋势图', switchMarkup('trend', preferences.trend, false, 'tray'))
    + '</section>' + message + '</div>';
}

function fatalStateMarkup() {
  return '<section class="fatal-state">' + icon('alert')
    + '<strong>无法连接本地服务</strong><p>' + esc(state.loadError || '未知错误') + '</p>'
    + '<button type="button" class="button" data-action="retry">' + icon('refresh') + '<span>重新连接</span></button></section>';
}

function renderContent() {
  const content = el('content');
  if (!content) return;
  if (state.loading && !state.hasLoaded) {
    content.innerHTML = '<section class="loading-state"><span class="spinner"></span><span>正在读取本地数据</span></section>';
    return;
  }
  if (state.loadError && !state.hasLoaded) {
    content.innerHTML = fatalStateMarkup();
    return;
  }
  const pages = {
    overview: overviewPage,
    resets: resetsPage,
    local: localCodexPage,
    accounts: accountsPage,
    settings: settingsPage,
  };
  content.innerHTML = (pages[state.activePage] || overviewPage)();
}

function headerButton(iconName, title, attributes, extraClass) {
  return '<button type="button" class="icon-button' + (extraClass ? ' ' + extraClass : '')
    + '" ' + attributes + ' title="' + esc(title) + '" aria-label="' + esc(title) + '">'
    + icon(iconName) + '</button>';
}

function headerMetadata() {
  if (state.activePage === 'settings') return ['settings', 'LLM Meter 设置', '应用与托盘偏好'];
  if (state.activePage === 'accounts') return ['users', '账户与 Provider', '登录、Relay 与本地代理'];
  if (state.activePage === 'resets') return ['restore', 'Reset 额度', 'ChatGPT 订阅重置机会'];
  if (state.activePage === 'local') return ['terminal', '本地 Codex', '运行会话、模型与 API 等价费用'];
  if (state.loadError && !state.hasLoaded) return ['chart-line', 'LLM Meter', 'daemon 离线'];
  if (!connections().length) return ['chart-line', '尚未添加连接', '可在设置中登录'];
  if (connections().length > 1) return ['chart-line', '多账号概览', connections().length + ' 个账号'];
  const connection = selectedConnection();
  return ['chart-line', connection?.display_name || 'LLM Meter', state.loadError ? 'daemon 离线' : statusText(connection?.status)];
}

function updateHeader() {
  const metadata = headerMetadata();
  const offline = Boolean(state.loadError);
  el('header-mark').innerHTML = icon(metadata[0]);
  el('header-mark').classList.toggle('is-error', offline);
  el('header-title').textContent = metadata[1];
  el('header-subtitle').textContent = metadata[2];
  el('header-subtitle').classList.toggle('is-error', offline);
  const actions = [];
  if (state.activePage === 'overview') {
    if (hasSubscription()) {
      actions.push(headerButton('restore', 'Reset 额度', 'data-page="resets"'));
    }
    actions.push(headerButton('terminal', '本地 Codex', 'data-page="local"'));
    actions.push(headerButton('users', '账户管理', 'data-page="accounts"'));
    actions.push(headerButton('settings', '设置', 'data-page="settings"'));
  } else {
    actions.push(headerButton('arrow-left', '返回概览', 'data-page="overview"'));
  }
  actions.push(headerButton('refresh', '强制刷新全部数据', 'data-action="refresh-all"',
    state.busyAction === 'refresh-all' ? 'is-spinning' : ''));
  actions.push(headerButton('x', '关闭', 'data-action="close"'));
  el('header-actions').innerHTML = actions.join('');
  el('header-actions').querySelectorAll('button').forEach(button => {
    if (state.busyAction && button.dataset.action === 'refresh-all') button.disabled = true;
  });
}

function loginOptions() {
  const result = [];
  for (const provider of state.providers) {
    for (const connectionType of provider.connection_types || []) {
      const schemes = connectionType.auth_schemes || [];
      const oauth = schemes.find(scheme => scheme === 'oauth_browser' || scheme === 'oauth_device_code');
      const apiKey = schemes.find(scheme => scheme === 'admin_api_key' || scheme === 'api_key');
      const authScheme = oauth || apiKey;
      if (!authScheme) continue;
      result.push({
        providerId: provider.provider_id,
        providerName: provider.display_name || provider.provider_id,
        connectionType: connectionType.id,
        label: connectionType.display_name || provider.display_name || provider.provider_id,
        authScheme,
        oauth: Boolean(oauth),
        apiKey: Boolean(apiKey),
        relay: provider.provider_id === 'relay' && connectionType.id === 'openai_compatible_proxy',
        relayProfile: provider.provider_id === 'relay' ? connectionType.id : '',
      });
    }
  }
  return result;
}

function modalShell(title, body, closeDisabled) {
  return '<div class="modal-backdrop" data-modal-backdrop><section class="modal-card" role="dialog" aria-modal="true" aria-label="'
    + esc(title) + '"><header class="modal-head"><h2>' + esc(title) + '</h2>'
    + '<button type="button" class="icon-button" data-action="close-modal" title="关闭" aria-label="关闭"'
    + (closeDisabled ? ' disabled' : '') + '>' + icon('x') + '</button></header>'
    + '<div class="modal-body">' + body + '</div></section></div>';
}

function addChoiceModalMarkup() {
  const options = loginOptions();
  let list = '';
  if (state.providerLoading) {
    list = '<div class="loading-state"><span class="spinner"></span><span>正在读取 Provider</span></div>';
  } else if (!options.length) {
    list = '<div class="notice-bar error">' + icon('alert')
      + '<span>未检测到可登录的 Provider，请检查 daemon 是否已启动或版本是否匹配。</span></div>';
  } else {
    list = '<ul class="provider-options">' + options.map((option, index) => '<li><button type="button" class="provider-option"'
      + ' data-action="select-provider" data-provider-index="' + index + '">'
      + icon(option.oauth ? 'login' : 'key')
      + '<span class="provider-option-copy"><strong>' + esc(option.label) + '</strong></span>'
      + '</button></li>').join('') + '</ul>';
  }
  return modalShell('添加账号', '<p class="modal-copy">选择要登录的账号类型</p>' + list, Boolean(state.busyAction));
}

function credentialModalMarkup(modal) {
  const option = modal.option;
  const values = modal.values || {};
  const genericRelayFields = option.relay
    ? '<input name="displayName" value="' + esc(values.displayName || 'My Provider')
      + '" maxlength="80" placeholder="Provider 名称" aria-label="Provider 名称" required>'
      + '<input name="origin" type="url" value="' + esc(values.origin || '')
      + '" placeholder="https://api.example.com" aria-label="Base URL" required>'
      + '<input name="listen" value="' + esc(values.listen || '127.0.0.1:18456')
      + '" placeholder="127.0.0.1:18456" aria-label="本地监听地址" required>'
    : '';
  const newApiFields = option.relayProfile === 'new_api'
    ? '<input name="origin" type="url" value="' + esc(values.origin || '')
      + '" placeholder="https://api.example.com" aria-label="Base URL" required>'
    : '';
  const error = modal.error
    ? '<div class="notice-bar error">' + icon('alert') + '<span>' + esc(modal.error) + '</span></div>'
    : '';
  const body = '<form class="form" id="connection-credential-form">'
    + '<strong class="credential-title">' + esc(option.relay ? 'OpenAI 兼容 Provider' : option.label) + '</strong>'
    + '<p class="modal-copy">' + esc(option.relay
      ? '输入 Provider 名称、Base URL 和上游 API Key。系统会验证 /v1/models，然后创建本地代理。'
      : '请输入 API Key，提交后会保存到 Windows 凭据管理器。') + '</p>'
    + error + genericRelayFields + newApiFields
    + '<input name="secret" type="password" placeholder="sk-..." aria-label="API Key" autocomplete="off" required>'
    + (option.relay ? '<p class="modal-copy">连接成功后可直接在 LLM Meter 中启动代理并管理多个客户端凭证。</p>' : '')
    + '<div class="form-actions"><button type="button" class="button" data-action="close-modal">取消</button>'
    + '<button type="submit" class="button primary"' + (state.busyAction ? ' disabled' : '') + '>'
    + (state.busyAction ? '<span class="spinner"></span><span>正在验证</span>' : icon('check') + '<span>确认添加</span>')
    + '</button></div></form>';
  return modalShell('添加账号', body, Boolean(state.busyAction));
}

function authModalMarkup(modal) {
  const deviceCode = modal.userCode
    ? '<output class="device-code">' + esc(modal.userCode) + '</output>'
      + '<button type="button" class="button" data-action="copy" data-copy-value="' + esc(modal.userCode)
      + '">' + icon('copy') + '<span>复制设备码</span></button>'
    : '';
  const openButton = modal.url
    ? '<button type="button" class="button primary" data-action="open-auth-url" data-auth-url="' + esc(modal.url)
      + '">' + icon('external') + '<span>打开登录页面</span></button>'
    : '';
  const body = '<div class="auth-progress"><span class="item-mark">' + icon('login') + '</span>'
    + '<strong>' + esc(modal.userCode ? '在浏览器中输入设备码' : '在浏览器中完成登录') + '</strong>'
    + '<p>登录请求正在等待 Provider 确认，完成后 LLM Meter 会自动刷新。</p>'
    + deviceCode + openButton + '<span class="spinner"></span></div>';
  return modalShell('正在登录 ' + modal.label, body, false);
}

function proxyModalMarkup(modal) {
  if (modal.loading) {
    return modalShell('本地代理', '<div class="loading-state"><span class="spinner"></span><span>正在读取代理状态</span></div>', true);
  }
  const connection = connections().find(item => item.id === modal.connectionId);
  const credentials = (modal.credentials || []).filter(credential => !credential.disabled_at);
  const credentialList = credentials.length
    ? '<ul class="credential-list">' + credentials.map(credential => '<li class="credential-row"><span class="item-copy">'
      + '<strong class="item-title">' + esc(credential.display_name || 'Client') + '</strong>'
      + '<span class="item-detail">' + esc((credential.token_prefix || '') + '…') + '</span></span>'
      + '<button type="button" class="button" data-action="disable-credential" data-credential-id="'
      + esc(credential.id) + '"' + (state.busyAction ? ' disabled' : '') + '>撤销</button></li>').join('') + '</ul>'
    : '<p class="modal-copy">尚无客户端凭证；启动代理时会自动创建默认凭证。</p>';
  const reveal = modal.revealed
    ? '<div class="credential-reveal"><span>新 Token（已复制到剪贴板，仅显示一次）：</span><output>'
      + esc(modal.revealed) + '</output></div>'
    : '';
  const body = '<p class="proxy-state-text' + (modal.running ? ' running' : '') + '">'
    + esc((modal.running ? '运行中 · http://' : '已停止 · http://') + (modal.listen || '127.0.0.1:18456') + '/v1')
    + '</p><button type="button" class="button full primary" data-action="toggle-proxy"'
    + (state.busyAction ? ' disabled' : '') + '>' + icon(modal.running ? 'stop' : 'play')
    + '<span>' + (modal.running ? '停止代理' : '启动代理') + '</span></button>'
    + '<h3 class="modal-section-title">客户端凭证</h3>' + credentialList
    + '<form class="form" id="proxy-credential-form"><input name="displayName" value="Client" maxlength="80" placeholder="凭证名称" aria-label="凭证名称" required>'
    + '<input name="token" type="password" minlength="24" placeholder="自定义 Token（可选，至少 24 字符）" aria-label="自定义 Token">'
    + '<button type="submit" class="button full">' + icon('key') + '<span>创建客户端凭证</span></button></form>'
    + reveal;
  return modalShell((connection?.display_name || 'Provider') + ' · 本地代理', body, Boolean(state.busyAction));
}

function renderModal() {
  const root = el('modal-root');
  if (!root) return;
  if (!state.modal) {
    root.innerHTML = '';
    return;
  }
  if (state.modal.type === 'add-choice') root.innerHTML = addChoiceModalMarkup();
  else if (state.modal.type === 'credential') root.innerHTML = credentialModalMarkup(state.modal);
  else if (state.modal.type === 'auth') root.innerHTML = authModalMarkup(state.modal);
  else if (state.modal.type === 'proxy') root.innerHTML = proxyModalMarkup(state.modal);
  else root.innerHTML = '';
  requestAnimationFrame(() => {
    root.querySelectorAll('form').forEach(updateFormSubmitState);
    if (state.modal?.type === 'credential') root.querySelector('input')?.focus();
  });
}

function applyWidgetExpanded(expanded) {
  if (role !== 'widget') return;
  state.widgetExpanded = Boolean(expanded);
  document.body.dataset.expanded = String(state.widgetExpanded);
  if (!state.widgetExpanded) {
    state.modal = null;
    state.pendingRemoveId = '';
  }
  render();
  if (state.widgetExpanded && !state.providerCatalogLoaded && !state.providerLoading) {
    loadProviders();
  }
}

window.__llmMeterSetWidgetExpanded = applyWidgetExpanded;

function render() {
  normalizeSelection();
  if (role === 'widget') renderWidget();
  if (role === 'widget' && !state.widgetExpanded) {
    return;
  }
  updateHeader();
  renderContent();
  renderModal();
}

function showToast(message, type) {
  const root = el('toast-root');
  if (!root) return;
  clearTimeout(toastTimer);
  const error = type === 'error';
  root.innerHTML = '<div class="toast' + (error ? ' error' : '') + '">'
    + icon(error ? 'alert' : 'check') + '<span>' + esc(message) + '</span></div>';
  toastTimer = setTimeout(() => { root.innerHTML = ''; }, 3200);
}

function setPage(page) {
  const allowed = ['overview', 'resets', 'local', 'accounts', 'settings'];
  state.activePage = allowed.includes(page) ? page : 'overview';
  state.pendingRemoveId = '';
  state.actionMessage = '';
  render();
  el('content')?.scrollTo({ top: 0, behavior: 'auto' });
  if (state.activePage === 'settings') refreshAutostart();
}

async function loadSnapshot(manual) {
  if (state.loading && state.hasLoaded) return;
  state.loading = true;
  if (!state.hasLoaded) render();
  try {
    const snapshot = await invoke('snapshot');
    snapshot.connections = Array.isArray(snapshot.connections) ? snapshot.connections : [];
    snapshot.local_codex = snapshot.local_codex || {};
    state.snapshot = snapshot;
    state.hasLoaded = true;
    state.loadError = '';
    normalizeSelection();
    if (manual) showToast('本地状态已重新读取');
  } catch (error) {
    state.loadError = String(error?.message || error || '未知错误');
    if (manual) showToast('读取失败：' + state.loadError, 'error');
  } finally {
    state.loading = false;
    render();
    updateTrayTooltip();
  }
}

async function loadProviders() {
  state.providerLoading = true;
  try {
    const providers = await invoke('provider_catalog');
    state.providers = Array.isArray(providers) ? providers : [];
  } catch (_) {
    state.providers = [];
  } finally {
    state.providerLoading = false;
    state.providerCatalogLoaded = true;
    if (state.modal?.type === 'add-choice') renderModal();
  }
}

async function refreshAll() {
  if (state.busyAction) return;
  state.busyAction = 'refresh-all';
  state.actionMessage = '正在刷新全部连接与本地 Codex…';
  render();
  try {
    await invoke('refresh_all');
    state.actionMessage = '全部连接与本地 Codex 已刷新';
    showToast('全部数据已刷新');
    await loadSnapshot(false);
  } catch (error) {
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

async function refreshConnection(connectionId) {
  if (state.busyAction) return;
  state.busyAction = 'refresh-' + connectionId;
  state.actionMessage = '正在刷新连接…';
  render();
  try {
    await invoke('refresh_provider', { connectionId });
    state.actionMessage = '连接已刷新';
    showToast('连接已刷新');
    await loadSnapshot(false);
  } catch (error) {
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

async function removeConnection(connectionId) {
  if (state.pendingRemoveId !== connectionId) {
    state.pendingRemoveId = connectionId;
    state.actionMessage = '再次点击红色按钮确认删除';
    clearTimeout(removeTimer);
    removeTimer = setTimeout(() => {
      state.pendingRemoveId = '';
      render();
    }, 5000);
    render();
    return;
  }
  if (state.busyAction) return;
  state.busyAction = 'remove-' + connectionId;
  state.actionMessage = '正在删除连接…';
  render();
  try {
    await invoke('remove_connection', { connectionId });
    state.pendingRemoveId = '';
    state.actionMessage = '连接已删除';
    showToast('连接已删除');
    await loadSnapshot(false);
  } catch (error) {
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

function randomUuid() {
  if (window.crypto?.randomUUID) return window.crypto.randomUUID();
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, character => {
    const random = Math.random() * 16 | 0;
    const value = character === 'x' ? random : (random & 0x3 | 0x8);
    return value.toString(16);
  });
}

async function saveBudget(form) {
  const connectionId = form.dataset.budgetForm;
  const connection = connections().find(item => item.id === connectionId);
  const amount = form.elements.amount.value.trim();
  if (!amount || Number(amount) <= 0) {
    showToast('请输入有效的月度预算', 'error');
    return;
  }
  state.busyAction = 'budget-' + connectionId;
  state.actionMessage = '正在保存预算…';
  render();
  try {
    await invoke('set_budget', {
      budgetId: connection?.budget?.id || randomUuid(),
      connectionId,
      amount,
      currency: 'USD',
    });
    state.actionMessage = '月度预算已保存';
    showToast('月度预算已保存');
    await loadSnapshot(false);
  } catch (error) {
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

function openAddModal() {
  state.modal = { type: 'add-choice' };
  renderModal();
  if (!state.providers.length && !state.providerLoading) loadProviders();
}

async function selectProviderOption(index) {
  const option = loginOptions()[index];
  if (!option || state.busyAction) return;
  if (!option.oauth && option.apiKey) {
    state.modal = { type: 'credential', option, values: {} };
    renderModal();
    return;
  }
  state.busyAction = 'auth';
  state.actionMessage = '正在准备登录…';
  render();
  try {
    const challenge = await invoke('begin_auth', {
      providerId: option.providerId,
      connectionType: option.connectionType,
      displayName: option.label,
      authScheme: option.authScheme,
      settings: null,
    });
    const url = challenge.auth_url || challenge.verification_url || '';
    const challengeId = challenge.challenge_id || challenge.state;
    if (!challengeId) throw new Error('daemon 未返回登录状态');
    state.modal = {
      type: 'auth',
      label: option.label,
      url,
      userCode: challenge.user_code || '',
    };
    renderModal();
    if (url) await invoke('open_auth_url', { authUrl: url });
    await invoke('complete_auth', { challengeId, secret: null });
    state.modal = null;
    state.actionMessage = '登录成功，正在刷新用量…';
    showToast('登录成功');
    await loadSnapshot(false);
  } catch (error) {
    state.modal = null;
    state.actionMessage = '登录未完成：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

async function submitConnection(form) {
  const modal = state.modal;
  if (!modal || modal.type !== 'credential' || state.busyAction) return;
  const option = modal.option;
  const values = {
    displayName: form.elements.displayName?.value.trim() || option.label,
    origin: form.elements.origin?.value.trim() || '',
    listen: form.elements.listen?.value.trim() || '127.0.0.1:18456',
  };
  const secret = form.elements.secret.value;
  state.busyAction = 'add-connection';
  state.actionMessage = option.relay ? '正在验证模型接口并创建本地代理…' : '正在保存 API Key…';
  state.modal = Object.assign({}, modal, { values, error: '' });
  render();
  try {
    let settings = null;
    if (option.relay) {
      if (!values.origin || !secret) throw new Error('请输入 Base URL 和 API Key');
      const report = await invoke('validate_compatible', { origin: values.origin, secret });
      settings = {
        schema_version: 1,
        values: {
          profile: 'generic',
          origin: report.normalized_origin,
          collection_mode: 'proxy',
          listen: values.listen,
        },
      };
    } else if (option.relayProfile) {
      const origin = option.relayProfile === 'openrouter'
        ? 'https://openrouter.ai'
        : values.origin;
      if (!origin || !secret) throw new Error('请输入 Base URL 和 API Key');
      settings = {
        schema_version: 1,
        values: {
          profile: option.relayProfile,
          origin,
          collection_mode: 'pull',
          listen: values.listen,
        },
      };
    }
    const challenge = await invoke('begin_auth', {
      providerId: option.providerId,
      connectionType: option.connectionType,
      displayName: values.displayName || option.label,
      authScheme: option.authScheme,
      settings,
    });
    const challengeId = challenge.challenge_id || challenge.state;
    if (!challengeId) throw new Error('daemon 未返回凭据验证状态');
    const connection = await invoke('complete_auth', { challengeId, secret });
    state.modal = null;
    state.actionMessage = '账号已添加，数据正在刷新';
    showToast(option.relay ? 'Provider 已连接，本地代理已创建' : '账号已添加');
    await loadSnapshot(false);
    if (option.relay && connection?.id) await openProxy(connection.id);
  } catch (error) {
    state.modal = Object.assign({}, modal, {
      type: 'credential',
      values,
      error: String(error?.message || error),
    });
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
  } finally {
    state.busyAction = '';
    render();
  }
}

async function readProxy(connectionId, keepReveal) {
  const previous = state.modal?.type === 'proxy' && state.modal.connectionId === connectionId
    ? state.modal
    : {};
  const results = await Promise.all([
    invoke('proxy_action', {
      method: 'proxy/status',
      connectionId,
      displayName: null,
      credentialId: null,
      token: null,
    }),
    invoke('proxy_action', {
      method: 'proxy/credentials/list',
      connectionId,
      displayName: null,
      credentialId: null,
      token: null,
    }),
  ]);
  state.modal = {
    type: 'proxy',
    connectionId,
    loading: false,
    running: Boolean(results[0]?.running),
    listen: results[0]?.listen || previous.listen || '127.0.0.1:18456',
    credentials: Array.isArray(results[1]) ? results[1] : [],
    revealed: keepReveal ? previous.revealed || '' : '',
  };
}

async function openProxy(connectionId) {
  state.modal = { type: 'proxy', connectionId, loading: true, credentials: [] };
  renderModal();
  try {
    await readProxy(connectionId, false);
  } catch (error) {
    state.modal = null;
    showToast('无法读取代理状态：' + String(error?.message || error), 'error');
  }
  renderModal();
}

async function toggleProxy() {
  const modal = state.modal;
  if (!modal || modal.type !== 'proxy' || state.busyAction) return;
  state.busyAction = 'proxy-toggle';
  renderModal();
  try {
    await invoke('proxy_action', {
      method: modal.running ? 'proxy/stop' : 'proxy/start',
      connectionId: modal.connectionId,
      displayName: null,
      credentialId: null,
      token: null,
    });
    await readProxy(modal.connectionId, true);
    showToast(modal.running ? '代理已停止' : '代理已启动');
  } catch (error) {
    showToast('代理操作失败：' + String(error?.message || error), 'error');
  } finally {
    state.busyAction = '';
    renderModal();
  }
}

async function createProxyCredential(form) {
  const modal = state.modal;
  if (!modal || modal.type !== 'proxy' || state.busyAction) return;
  const displayName = form.elements.displayName.value.trim();
  const token = form.elements.token.value.trim() || null;
  state.busyAction = 'proxy-credential';
  renderModal();
  try {
    const result = await invoke('proxy_action', {
      method: 'proxy/credentials/create',
      connectionId: modal.connectionId,
      displayName,
      credentialId: null,
      token,
    });
    await readProxy(modal.connectionId, false);
    state.modal.revealed = result.token || '';
    if (result.token) {
      navigator.clipboard.writeText(result.token).catch(() => {});
    }
    showToast('客户端凭证已创建');
  } catch (error) {
    showToast('创建失败：' + String(error?.message || error), 'error');
  } finally {
    state.busyAction = '';
    renderModal();
  }
}

async function disableProxyCredential(credentialId) {
  const modal = state.modal;
  if (!modal || modal.type !== 'proxy' || state.busyAction) return;
  state.busyAction = 'proxy-credential';
  renderModal();
  try {
    await invoke('proxy_action', {
      method: 'proxy/credentials/disable',
      connectionId: modal.connectionId,
      displayName: null,
      credentialId,
      token: null,
    });
    await readProxy(modal.connectionId, true);
    showToast('客户端凭证已撤销');
  } catch (error) {
    showToast('撤销失败：' + String(error?.message || error), 'error');
  } finally {
    state.busyAction = '';
    renderModal();
  }
}

async function refreshAutostart() {
  if (state.autostart.loading) return;
  state.autostart.loading = true;
  render();
  try {
    const status = await invoke('autostart_action', { action: 'status' });
    state.autostart = {
      known: true,
      loading: false,
      enabled: status.enabled === true,
      active: status.active === true,
      installed: status.installed === true,
      state: status.state || 'unknown',
    };
  } catch (_) {
    state.autostart.known = false;
    state.autostart.loading = false;
  }
  render();
}

async function setAutostart(enabled) {
  if (state.busyAction) return;
  state.busyAction = 'autostart';
  state.actionMessage = enabled ? '正在启用开机自启动…' : '正在关闭开机自启动…';
  render();
  try {
    const status = await invoke('autostart_action', { action: enabled ? 'enable' : 'disable' });
    state.autostart = {
      known: true,
      loading: false,
      enabled: status.enabled === true,
      active: status.active === true,
      installed: status.installed === true,
      state: status.state || 'unknown',
    };
    state.actionMessage = enabled ? '开机自启动已启用' : '开机自启动已关闭';
    showToast(state.actionMessage);
  } catch (error) {
    state.actionMessage = '操作失败：' + String(error?.message || error);
    showToast(state.actionMessage, 'error');
    await refreshAutostart();
  } finally {
    state.busyAction = '';
    render();
  }
}

function lowestQuotaConnection() {
  let best = null;
  for (const connection of connections()) {
    const quota = connectionQuota(connection);
    if (!quota) continue;
    if (!best || Number(quota.remaining_ratio) < Number(best.quota.remaining_ratio)) {
      best = { connection, quota };
    }
  }
  return best?.connection || connections()[0] || null;
}

function sparkline(values) {
  const bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
  const maximum = Math.max.apply(null, values.concat([0]));
  if (maximum <= 0) return '';
  return values.map(value => bars[clamp(Math.round(Number(value || 0) / maximum * 7), 0, 7)]).join('');
}

function barSummary() {
  const preferences = state.trayPreferences;
  const connection = lowestQuotaConnection();
  const overview = selectedOverviewValues(connection);
  const parts = [];
  if (preferences.account && connection) parts.push(connection.display_name || 'Provider');
  if (preferences.quota && connectionQuotaPercent(connection) >= 0) {
    parts.push(connectionQuotaPercent(connection) + '%');
  }
  if (preferences.todayTokens && overview.tokens !== '—') {
    parts.push(overview.tokens + ' tok');
  }
  if (preferences.todayCost && overview.cost !== '—') {
    parts.push(overview.cost);
  }
  if (preferences.codexSessions) {
    parts.push((localCodex().active_sessions || []).length + ' Codex');
  }
  if (preferences.trend && connection) {
    const context = periodContext();
    const trend = sparkline(connectionPeriodValues(connection, context.keys, context.now));
    if (trend) parts.push(trend);
  }
  const stateLine = connection ? statusText(connection.status) : '尚未添加连接';
  return { connection, parts, stateLine };
}

function composeWidgetLabel() {
  if (state.loadError && !state.hasLoaded) return 'LLM offline';
  const summary = barSummary();
  return summary.parts.length ? summary.parts.join(' ') : 'LLM Meter';
}

function renderWidget() {
  const root = el('widget-shell');
  const label = el('widget-label');
  const toggle = el('widget-toggle');
  if (!root || !label || !toggle) return;
  const text = state.loading && !state.hasLoaded ? 'LLM Meter' : composeWidgetLabel();
  const tooltip = state.loadError
    ? 'LLM Meter\n' + state.loadError
    : composeTrayTooltip();
  label.textContent = text;
  root.title = tooltip;
  root.setAttribute('aria-label', tooltip.replace(/\n/g, ' · '));
  root.classList.toggle('is-error', Boolean(state.loadError));
  root.classList.toggle('is-expanded', state.widgetExpanded);
  const toggleLabel = state.widgetExpanded ? '收起为迷你条' : '展开完整信息';
  toggle.title = toggleLabel;
  toggle.setAttribute('aria-label', toggleLabel);
}

function composeTrayTooltip() {
  const summary = barSummary();
  const { parts, stateLine } = summary;
  return 'LLM Meter\n' + (parts.length ? parts.join(' · ') : stateLine);
}

async function updateTrayTooltip() {
  if (!state.hasLoaded) return;
  const tooltip = composeTrayTooltip();
  if (tooltip === lastTrayTooltip) return;
  lastTrayTooltip = tooltip;
  try {
    await invoke('update_tray_tooltip', { tooltip });
  } catch (_) {
    lastTrayTooltip = '';
  }
}

function saveTrayPreferences() {
  try {
    localStorage.setItem('llm-meter-tray-preferences', JSON.stringify(state.trayPreferences));
  } catch (_) {}
  lastTrayTooltip = '';
  updateTrayTooltip();
}

function attachSurfaceInteractions() {
  if (role === 'widget') {
    const shell = el('widget-shell');
    const toggle = el('widget-toggle');
    if (!shell || !toggle) return;
    let pointer = null;
    const resetPointer = () => { pointer = null; };

    shell.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      if (event.target.closest('#widget-toggle')) return;
      pointer = {
        id: event.pointerId,
        x: event.screenX,
        y: event.screenY,
        dragging: false,
      };
      shell.setPointerCapture?.(event.pointerId);
    });
    shell.addEventListener('pointermove', event => {
      if (!pointer || pointer.id !== event.pointerId || pointer.dragging) return;
      if (Math.hypot(event.screenX - pointer.x, event.screenY - pointer.y) < 5) return;
      pointer.dragging = true;
      invoke('drag_surface').catch(() => {});
    });
    shell.addEventListener('pointerup', event => {
      if (!pointer || pointer.id !== event.pointerId) return;
      const shouldToggle = !pointer.dragging;
      resetPointer();
      if (shouldToggle) invoke('activate', { action: 'toggle' }).catch(() => {});
    });
    shell.addEventListener('pointercancel', resetPointer);
    toggle.addEventListener('pointerdown', event => event.stopPropagation());
    toggle.addEventListener('pointerup', event => event.stopPropagation());
    toggle.addEventListener('click', event => {
      event.stopPropagation();
      invoke('activate', { action: 'toggle' }).catch(() => {});
    });
    shell.addEventListener('contextmenu', event => {
      event.preventDefault();
      invoke('activate', { action: 'main' }).catch(() => {});
    });
  }

  const header = document.querySelector('.window-header');
  header?.addEventListener('pointerdown', event => {
    if (event.button !== 0 || event.target.closest('button, input, select, a')) return;
    invoke('drag_surface').catch(() => {});
  });
}

async function copyText(value) {
  try {
    await navigator.clipboard.writeText(value);
    showToast('已复制到剪贴板');
  } catch (error) {
    showToast('复制失败：' + String(error?.message || error), 'error');
  }
}

function updateFormSubmitState(form) {
  const submit = form?.querySelector('button[type="submit"]');
  if (submit) submit.disabled = Boolean(state.busyAction) || !form.checkValidity();
}

async function collapseWidget() {
  if (role !== 'widget' || !state.widgetExpanded) return;
  try {
    await invoke('set_widget_expanded', { expanded: false });
    applyWidgetExpanded(false);
  } catch (error) {
    showToast('无法收起：' + String(error?.message || error), 'error');
  }
}

document.addEventListener('click', async event => {
  const pageButton = event.target.closest('[data-page]');
  if (pageButton) {
    setPage(pageButton.dataset.page);
    return;
  }

  const actionTarget = event.target.closest('[data-action]');
  const action = actionTarget?.dataset.action;
  if (action === 'refresh-all') {
    await refreshAll();
    return;
  }
  if (action === 'retry') {
    await loadSnapshot(true);
    return;
  }
  if (action === 'close') {
    if (role === 'widget') await collapseWidget();
    else await invoke('close_surface').catch(() => {});
    return;
  }
  if (action === 'open-add') {
    openAddModal();
    return;
  }
  if (action === 'close-modal') {
    if (!state.busyAction || state.modal?.type === 'auth') {
      state.modal = null;
      renderModal();
    }
    return;
  }
  if (action === 'select-provider') {
    await selectProviderOption(Number(actionTarget.dataset.providerIndex));
    return;
  }
  if (action === 'refresh-connection') {
    await refreshConnection(actionTarget.dataset.connectionId);
    return;
  }
  if (action === 'remove-connection') {
    await removeConnection(actionTarget.dataset.connectionId);
    return;
  }
  if (action === 'open-proxy') {
    await openProxy(actionTarget.dataset.connectionId);
    return;
  }
  if (action === 'toggle-proxy') {
    await toggleProxy();
    return;
  }
  if (action === 'disable-credential') {
    await disableProxyCredential(actionTarget.dataset.credentialId);
    return;
  }
  if (action === 'copy') {
    await copyText(actionTarget.dataset.copyValue || '');
    return;
  }
  if (action === 'open-auth-url') {
    await invoke('open_auth_url', { authUrl: actionTarget.dataset.authUrl })
      .catch(error => showToast(String(error?.message || error), 'error'));
    return;
  }
  if (action === 'open-release') {
    await invoke('open_release').catch(error => showToast(String(error?.message || error), 'error'));
    return;
  }

  const accountButton = event.target.closest('[data-select-account]');
  if (accountButton) {
    state.selectedConnectionId = accountButton.dataset.selectAccount;
    render();
    updateTrayTooltip();
    return;
  }

  const card = event.target.closest('[data-select-card]');
  if (card && !event.target.closest('button, input, form')) {
    state.selectedConnectionId = card.dataset.selectCard;
    render();
    return;
  }

  if (event.target.matches('[data-modal-backdrop]')
    && (!state.busyAction || state.modal?.type === 'auth')) {
    state.modal = null;
    renderModal();
  }
});

document.addEventListener('submit', async event => {
  event.preventDefault();
  if (event.target.id === 'connection-credential-form') {
    await submitConnection(event.target);
    return;
  }
  if (event.target.matches('[data-budget-form]')) {
    await saveBudget(event.target);
    return;
  }
  if (event.target.id === 'proxy-credential-form') {
    await createProxyCredential(event.target);
  }
});

document.addEventListener('change', async event => {
  if (event.target.matches('[data-autostart-toggle]')) {
    await setAutostart(event.target.checked);
    return;
  }
  const key = event.target.dataset.trayKey;
  if (key && Object.prototype.hasOwnProperty.call(state.trayPreferences, key)) {
    state.trayPreferences[key] = event.target.checked;
    saveTrayPreferences();
  }
});

document.addEventListener('input', event => {
  const form = event.target.closest('form');
  if (form) updateFormSubmitState(form);
});

document.addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  if (state.modal && (!state.busyAction || state.modal.type === 'auth')) {
    state.modal = null;
    renderModal();
    return;
  }
  if (state.activePage !== 'overview') {
    setPage('overview');
    return;
  }
  if (role === 'widget' && state.widgetExpanded) {
    collapseWidget();
  }
});

window.addEventListener('storage', event => {
  if (event.key !== 'llm-meter-tray-preferences') return;
  state.trayPreferences = readTrayPreferences();
  render();
  updateTrayTooltip();
});

async function bootstrap() {
  render();
  attachSurfaceInteractions();
  if (role === 'widget') {
    try {
      applyWidgetExpanded(await invoke('widget_expanded'));
    } catch (_) {}
  }
  const initialLoads = [loadSnapshot(false)];
  if (role === 'main') initialLoads.push(loadProviders());
  await Promise.allSettled(initialLoads);
  setInterval(() => {
    if (!state.busyAction && !state.modal && !['accounts', 'settings'].includes(state.activePage)) {
      loadSnapshot(false);
    }
  }, 5000);
}

bootstrap();
