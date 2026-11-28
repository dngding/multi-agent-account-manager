import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";
import type {
  AccountKey,
  AccountRecord,
  InstallUpdateOutcome,
  ManagerSnapshot,
  ProviderId,
  ProviderInfo,
  ProxyGateAssignment,
  ProxyGateEntry,
  ProxyGateProbeEntry,
  ProxyGateProbeSnapshot,
  ProxyGateSnapshot,
  ProxyPoolBinding,
  ProxyPoolEntry,
  ProxyPoolHealthEntry,
  ProxyPoolHealthSnapshot,
  ProxyPoolSnapshot,
  QuotaDetail,
  UpdateCheckResult,
  UpdateProgressEvent,
} from "./types";

type ModalMode =
  | { kind: "none" }
  | { kind: "add"; providerId: ProviderId; draft: string; proxyDraft: string; error: string | null }
  | { kind: "rename"; account: AccountRecord; draft: string }
  | { kind: "proxy"; account: AccountRecord; draft: string; error: string | null }
  | { kind: "remove"; account: AccountRecord }
  | { kind: "update" };

const PROVIDER_LOGOS: Record<ProviderId, { src: string; fullBleed: boolean }> = {
  muse: { src: "/logos/muse.png", fullBleed: true },
  hark: { src: "/logos/hark.svg", fullBleed: false },
};

const ICON_EXTERNAL = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>`;
const ICON_REFRESH = `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M21.5 2v6h-6M21.34 15.57a10 10 0 1 1-.57-8.38l5.67-5.67"/></svg>`;
const ICON_EDIT = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9"/><path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"/></svg>`;
const ICON_TRASH = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>`;
const ICON_NETWORK = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>`;
const ICON_USER_PLUS = `<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><line x1="19" y1="8" x2="19" y2="14"/><line x1="22" y1="11" x2="16" y2="11"/></svg>`;
const ICON_UPDATE = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>`;

let snapshot: ManagerSnapshot | null = null;
let modal: ModalMode = { kind: "none" };
let optimisticSwitching: AccountKey | null = null;
let busy = false;
let modalSubmitting = false;
let lastRenderedUpdateStage: string | null = null;
const copyingCookies = new Set<string>();

interface ProxyPickerState {
  mode: "pool" | "manual" | "gate";
  pool: ProxyPoolSnapshot | null;
  selected: ProxyPoolBinding | null;
  loading: boolean;
  search: string;
  sourceDraft: string;
  sourceTouched: boolean;
  sourceSettingsOpen: boolean;
  selectionWarning: string | null;
  requestSeq: number;
  health: Map<string, ProxyPoolHealthEntry>;
  healthLoading: boolean;
  healthError: string | null;
  healthCheckedAtMs: number | null;
  healthSeq: number;
  healthRevision: string | null;
  gate: ProxyGateSnapshot | null;
  gateLoading: boolean;
  gateError: string | null;
  gateProbing: boolean;
  gateProbeError: string | null;
  gateProbeSeq: number;
  gateSearch: string;
  gateSourceDraft: string;
  gateSelectedEntryId: string | null;
  gateActionLoading: boolean;
  gateRequestSeq: number;
}

let proxyPicker: ProxyPickerState | null = null;
let proxyGateSnapshot: ProxyGateSnapshot | null = null;
let proxyGateError: string | null = null;

function hasProxyPicker(): boolean {
  return snapshot?.platform === "windows" && (modal.kind === "add" || modal.kind === "proxy");
}

function proxyEntryUsers(entry: ProxyPoolEntry): AccountRecord[] {
  const endpoint = normalizedProxyEndpoint(entry.proxyUrl);
  return (snapshot?.accounts ?? []).filter((account) => {
    if (modal.kind === "proxy" && isSameKey(modal.account, account)) return false;
    return Boolean(
      (account.proxyUrl && normalizedProxyEndpoint(account.proxyUrl) === endpoint) ||
      account.proxyPoolBinding?.fingerprint === entry.fingerprint
    );
  });
}

function normalizedProxyEndpoint(value: string): string {
  try {
    const url = new URL(value);
    const host = url.hostname.toLowerCase();
    const normalizedHost = host === "localhost" || host === "[::1]" ? "127.0.0.1" : host;
    return `${normalizedHost}:${url.port}`;
  } catch {
    return value;
  }
}

function proxyPickerProviderId(): ProviderId | null {
  if (modal.kind === "add") return modal.providerId;
  if (modal.kind === "proxy") return modal.account.providerId;
  return null;
}

function proxyHealthLabel(health: ProxyPoolHealthEntry | undefined, probing: boolean): string {
  if (health?.available) return `可用 · ${health.latencyMs} ms`;
  if (health?.handshakeOnly) return `仅握手可用 · ${health.latencyMs} ms`;
  if (health) return "不可用";
  return probing ? "待检测" : "未检测";
}

function proxyGateHealthRank(entry: ProxyGateEntry): number {
  if (entry.health === "available") return 0;
  if (entry.health === "handshake_only") return 1;
  if (entry.health === "unavailable") return 2;
  return 3;
}

function proxyGateHealthLabel(
  health: string | null | undefined,
  latencyMs: number | null | undefined,
  fallback = "未检测",
): string {
  const latency = typeof latencyMs === "number" ? ` · ${latencyMs} ms` : "";
  switch (health) {
    case "available":
      return `可用${latency}`;
    case "handshake_only":
      return `仅握手可用${latency}`;
    case "unavailable":
      return "不可用";
    default:
      return fallback;
  }
}

function proxyGateCurrentAssignment(picker: ProxyPickerState): ProxyGateAssignment | null {
  if (modal.kind !== "proxy" || !picker.gate) return null;
  const account = modal.account;
  return picker.gate.assignments.find(
    (assignment) =>
      assignment.providerId === account.providerId &&
      assignment.uuid === account.uuid,
  ) ?? null;
}

function proxyGateOptions(picker: ProxyPickerState): string {
  const query = picker.gateSearch.trim().toLocaleLowerCase();
  const entries = picker.gate?.entries ?? [];
  const options = entries
    .filter((entry) =>
      entry.entryId === picker.gateSelectedEntryId ||
      `${entry.displayName} ${entry.entryId} ${entry.type}`.toLocaleLowerCase().includes(query)
    )
    .map((entry, index) => ({ entry, index }))
    .sort((left, right) => {
      const rank = proxyGateHealthRank(left.entry) - proxyGateHealthRank(right.entry);
      if (rank !== 0) return rank;
      if (
        typeof left.entry.latencyMs === "number" &&
        typeof right.entry.latencyMs === "number"
      ) {
        return left.entry.latencyMs - right.entry.latencyMs || left.index - right.index;
      }
      return left.index - right.index;
    });

  return `<option value="">请选择一个 Proxy Gate 入口</option>${options.map(({ entry }) => {
    const users = (picker.gate?.assignments ?? []).filter(
      (assignment) => assignment.entryId === entry.entryId,
    );
    const usage = users.length ? ` · ${users.length} 号已绑定` : "";
    const chainDepth = entry.chainDepth ?? 0;
    const chain = chainDepth > 0 ? ` · ${chainDepth} 层链` : "";
    const health = proxyGateHealthLabel(entry.health, entry.latencyMs);
    return `<option value="${escapeHtml(entry.entryId)}"${entry.entryId === picker.gateSelectedEntryId ? " selected" : ""}>${escapeHtml(`${health} · ${entry.displayName} · ${entry.type}${chain}${usage}`)}</option>`;
  }).join("")}`;
}

function proxyGateStatus(picker: ProxyPickerState): string {
  if (picker.gateLoading) return "正在读取 Proxy Gate 状态与入口…";
  if (picker.gateError) return `Proxy Gate 读取失败：${picker.gateError}`;
  if (picker.gateProbing) {
    return `${picker.gate?.entries.length ?? 0} 个入口 · 正在通过每个上游实际探测页面，可用入口完成后会自动置顶…`;
  }
  const gate = picker.gate;
  if (!gate) return "尚未读取 Proxy Gate 状态。";
  if (picker.gateProbeError) return `${gate.entries.length} 个入口；可用性检测失败：${picker.gateProbeError}`;
  if (gate.error) return gate.error;
  if (gate.status?.error) return gate.status.error;
  if (!gate.available) {
    return "未找到可用的 proxy-gate.exe 或配置文件。可通过 PROXY_GATE_EXE / PROXY_GATE_CONFIG 指定路径。";
  }
  const runtime = gate.running ? "运行中" : "已停止";
  const available = gate.entries.filter((entry) => entry.health === "available").length;
  const handshakeOnly = gate.entries.filter((entry) => entry.health === "handshake_only").length;
  const unavailable = gate.entries.filter((entry) => entry.health === "unavailable").length;
  const unknown = gate.entries.length - available - handshakeOnly - unavailable;
  return `${runtime} · ${gate.entries.length} 个入口（可用 ${available} / 仅握手 ${handshakeOnly} / 不可用 ${unavailable} / 未检测 ${unknown}）· ${gate.assignments.length} 个账号绑定`;
}

function proxyGateSelectedDetail(picker: ProxyPickerState): string {
  if (picker.gateError) return "修正 Proxy Gate 路径或启动问题后再选择入口。";
  const entryId = picker.gateSelectedEntryId;
  const entry = picker.gate?.entries.find((item) => item.entryId === entryId);
  if (!entry) return "保存后会为该账号创建固定的本机 191xx 入口，并在整链指纹变化时 fail-closed。";
  const assignments = (picker.gate?.assignments ?? []).filter(
    (assignment) => assignment.entryId === entry.entryId,
  );
  const parts = [
    `类型 ${entry.type}`,
    `指纹 ${entry.fingerprint.slice(0, 12)}…`,
    (entry.chainDepth ?? 0) > 0 ? `静态链 ${entry.chainDepth} 层` : "单入口",
    proxyGateHealthLabel(entry.health, entry.latencyMs),
  ];
  if (assignments.length) {
    parts.push(`已绑定：${assignments.map((item) => item.accountAlias || item.uuid.slice(0, 8)).join("、")}`);
  }
  if (entry.message) parts.push(entry.message);
  return parts.join("；");
}

function proxyPoolOptions(picker: ProxyPickerState): string {
  const query = picker.search.trim().toLocaleLowerCase();
  const entries = picker.pool?.error ? [] : picker.pool?.entries ?? [];
  const options = entries
    .filter((entry) =>
      entry.id === picker.selected?.entryId || `${entry.label} ${entry.proxyUrl}`.toLocaleLowerCase().includes(query)
    )
    .map((entry, index) => ({
      entry,
      index,
      health: picker.health.get(entry.id),
    }))
    .sort((left, right) => {
      const healthRank = (health: ProxyPoolHealthEntry | undefined): number => {
        if (!health) return 1;
        if (health.available) return 0;
        return health.handshakeOnly ? 1 : 2;
      };
      const leftRank = healthRank(left.health);
      const rightRank = healthRank(right.health);
      if (leftRank !== rightRank) return leftRank - rightRank;
      if (left.health && right.health && leftRank < 2) {
        return left.health.latencyMs - right.health.latencyMs || left.index - right.index;
      }
      return left.index - right.index;
    });
  return `<option value="">请选择一个代理，不会自动分配</option>${options.map((entry) => {
    const users = proxyEntryUsers(entry.entry);
    const usage = users.length ? ` · 本应用 ${users.length} 号已用` : "";
    const duplicate = entry.entry.duplicateCount > 1 ? ` · 同目标 ${entry.entry.duplicateCount} 入口` : "";
    const availability = proxyHealthLabel(entry.health, picker.healthLoading);
    return `<option value="${escapeHtml(entry.entry.id)}"${entry.entry.id === picker.selected?.entryId ? " selected" : ""}>${escapeHtml(`${availability} · ${entry.entry.label} · ${entry.entry.proxyUrl}${usage}${duplicate}`)}</option>`;
  }).join("")}`;
}

function proxyPoolStatus(picker: ProxyPickerState): string {
  if (picker.loading) return "正在读取最新代理池…";
  if (picker.pool?.error) return picker.pool.error;
  if (!picker.pool?.sourcePath) return "未找到代理池，请在“代理池来源”中选择或填写现有本地 YAML 文件。";
  if (!picker.pool.entries.length) return "此来源没有可选的固定代理入口。可更换来源或使用手动地址。";
  const available = picker.pool.entries.filter((entry) => picker.health.get(entry.id)?.available).length;
  const handshakeOnly = picker.pool.entries.filter((entry) => {
    const health = picker.health.get(entry.id);
    return health && !health.available && health.handshakeOnly;
  }).length;
  const failed = picker.pool.entries.filter((entry) => {
    const health = picker.health.get(entry.id);
    return health && !health.available && !health.handshakeOnly;
  }).length;
  const unchecked = picker.pool.entries.length - available - handshakeOnly - failed;
  if (picker.healthError) return `${picker.pool.entries.length} 个入口；可用性检测失败：${picker.healthError}`;
  if (picker.healthLoading) {
    if (available || handshakeOnly || failed) {
      return `可用 ${available} · 仅握手 ${handshakeOnly} · 不可用 ${failed} · 待复检 ${unchecked}；可用入口会排在前面`;
    }
    return `${picker.pool.entries.length} 个入口 · 正在检测代理可用性，完成后可用入口会自动排在前面`;
  }
  const checked = picker.healthCheckedAtMs
    ? new Date(picker.healthCheckedAtMs).toLocaleTimeString("zh-CN", { hour12: false })
    : new Date(picker.pool.checkedAtMs).toLocaleTimeString("zh-CN", { hour12: false });
  return `可用 ${available} · 仅握手 ${handshakeOnly} · 不可用 ${failed} · 未检测 ${unchecked} · ${checked} 检测；可用入口已置顶`;
}

function proxyPoolSelectedDetail(picker: ProxyPickerState): string {
  if (picker.selectionWarning) return picker.selectionWarning;
  const entry = picker.pool?.entries.find((item) => item.id === picker.selected?.entryId);
  if (!entry) return "选择后固定绑定此入口；代理池更新不会自动改选其他入口。";
  const users = proxyEntryUsers(entry);
  const parts = [users.length ? `本应用已用：${users.map((account) => account.alias).join("、")}` : "本应用暂无其他账号使用此端口或目标"];
  const health = picker.health.get(entry.id);
  if (health?.available) parts.push(`代理页面请求已通过，延迟 ${health.latencyMs} ms`);
  if (health?.handshakeOnly) {
    parts.push(`代理握手可用但页面请求失败，延迟 ${health.latencyMs} ms；打开前还会重新检查`);
  }
  if (health && !health.available && !health.handshakeOnly) {
    parts.push("当前代理页面请求和握手均失败，打开前还会重新检查");
  }
  if (entry.duplicateCount > 1) parts.push(`同一目标共有 ${entry.duplicateCount} 个入口`);
  return parts.join("；");
}

function renderProxyPicker(manualId: string, manualDraft: string): string {
  const picker = proxyPicker;
  if (!picker) return "";
  const poolMode = picker.mode === "pool";
  const gateMode = picker.mode === "gate";
  const gateAvailable = modal.kind === "proxy";
  const currentAssignment = proxyGateCurrentAssignment(picker);
  const gate = picker.gate;
  return `
    <div class="proxy-mode-switch" role="group" aria-label="代理配置方式">
      <button type="button" class="btn ${poolMode ? "selected" : ""}" data-proxy-mode="pool" aria-pressed="${poolMode}">静态代理池</button>
      <button type="button" class="btn ${gateMode ? "selected" : ""}" data-proxy-mode="gate" aria-pressed="${gateMode}"${gateAvailable ? "" : ` disabled title="请先保存账号，再进入此账号的代理配置绑定 Proxy Gate"`}>Proxy Gate</button>
      <button type="button" class="btn ${picker.mode === "manual" ? "selected" : ""}" data-proxy-mode="manual" aria-pressed="${picker.mode === "manual"}">手动地址</button>
    </div>
    ${gateMode ? `
      <div class="proxy-gate-panel">
        <div class="proxy-gate-runtime">
          <div>
            <strong>${gate?.running ? "Gate 运行中" : gate?.available ? "Gate 已停止" : "Gate 尚未就绪"}</strong>
            <span>${gate?.configPath ? escapeHtml(gate.configPath) : "等待配置"}</span>
          </div>
          <div class="proxy-gate-actions">
            <button id="proxy-gate-refresh" class="btn" type="button"${picker.gateLoading || picker.gateActionLoading ? " disabled" : ""}>刷新</button>
            <button id="proxy-gate-start" class="btn" type="button"${!gate?.available || gate.running || picker.gateActionLoading ? " disabled" : ""}>启动</button>
            <button id="proxy-gate-stop" class="btn" type="button"${!gate?.available || !gate.running || picker.gateActionLoading ? " disabled" : ""}>停止</button>
          </div>
        </div>
        <label class="modal-field-label" for="proxy-gate-search">搜索入口</label>
        <input id="proxy-gate-search" class="modal-input" placeholder="名称、入口 ID 或类型" value="${escapeHtml(picker.gateSearch)}" />
        <label class="modal-field-label" for="proxy-gate-select">选择固定入口</label>
        <select id="proxy-gate-select" class="modal-input proxy-pool-select"${picker.gateLoading || picker.gateError || !gate?.entries.length ? " disabled" : ""}>${proxyGateOptions(picker)}</select>
        <div id="proxy-gate-status" class="modal-desc${picker.gateError || gate?.error ? " proxy-pool-warning" : ""}" role="status">${escapeHtml(proxyGateStatus(picker))}</div>
        <div id="proxy-gate-probe-progress" class="proxy-gate-probe-progress" role="progressbar" aria-label="正在探测 Proxy Gate 入口"${picker.gateProbing ? "" : " hidden"}><div class="update-progress-track"><div class="update-progress-fill indeterminate"></div></div><span>正在检测上游代理可用性，最多需要数秒</span></div>
        <div id="proxy-gate-selection-detail" class="modal-desc${currentAssignment && !currentAssignment.bindingValid ? " proxy-pool-warning" : ""}">${escapeHtml(proxyGateSelectedDetail(picker))}</div>
        <details id="proxy-gate-source-settings" class="proxy-pool-source">
          <summary>Proxy Gate 代理池来源</summary>
          <div id="proxy-gate-current-source" class="modal-desc proxy-pool-path">${escapeHtml(gate?.poolPath ?? "自动发现现有代理池")}</div>
          <input id="proxy-gate-source-path" class="modal-input" spellcheck="false" placeholder="C:/path/to/sidecar.yaml；留空自动发现" value="${escapeHtml(picker.gateSourceDraft)}" />
          <div class="proxy-pool-source-actions">
            <button id="proxy-gate-source-apply" class="btn" type="button"${picker.gateActionLoading ? " disabled" : ""}>应用并同步入口</button>
            <button id="proxy-gate-source-auto" class="btn" type="button"${picker.gateActionLoading ? " disabled" : ""}>自动发现</button>
          </div>
          <div class="modal-desc">换池会重新计算入口和指纹。已有账号绑定时 Gate 会拒绝换池，必须先逐账号解除绑定，避免旧账号被静默切到新代理。</div>
        </details>
        ${currentAssignment ? `
          <div class="proxy-gate-current ${currentAssignment.bindingValid && currentAssignment.allowed !== false ? "ready" : "invalid"}">
            <div>
              <strong>当前绑定：${escapeHtml(currentAssignment.displayName)}</strong>
              <span>${escapeHtml(currentAssignment.endpoint)} · ${escapeHtml(proxyGateHealthLabel(currentAssignment.health, currentAssignment.latencyMs))}</span>
              <span>${currentAssignment.bindingValid && currentAssignment.allowed !== false ? "绑定有效，打开账号时仍会重新预检" : escapeHtml(currentAssignment.error || "绑定失效或指纹变化，打开会被阻断")}</span>
            </div>
            <button id="proxy-gate-unassign" class="btn" type="button"${picker.gateActionLoading ? " disabled" : ""}>解除绑定</button>
          </div>
        ` : ""}
        <div class="modal-desc">保存后固定写入该账号的本机 191xx 入口。Gate 在上游不可用或整链指纹漂移时返回 503并切断现有连接，不会回退直连。</div>
      </div>
    ` : poolMode ? `
      <div class="proxy-pool-picker">
        <label class="modal-field-label" for="proxy-pool-search">搜索代理</label>
        <div class="proxy-pool-toolbar">
          <input id="proxy-pool-search" class="modal-input" placeholder="名称或端口" value="${escapeHtml(picker.search)}" />
          <button id="proxy-pool-refresh" class="btn" type="button"${picker.loading || picker.healthLoading ? " disabled" : ""}>刷新</button>
        </div>
        <label class="modal-field-label" for="proxy-pool-select">选择固定入口</label>
        <select id="proxy-pool-select" class="modal-input proxy-pool-select"${(!picker.pool && picker.loading) || picker.pool?.error ? " disabled" : ""}>${proxyPoolOptions(picker)}</select>
        <div id="proxy-pool-status" class="modal-desc${picker.pool?.error ? " proxy-pool-warning" : ""}" role="status">${escapeHtml(proxyPoolStatus(picker))}</div>
        <div id="proxy-pool-selection-detail" class="modal-desc${picker.selectionWarning ? " proxy-pool-warning" : ""}">${escapeHtml(proxyPoolSelectedDetail(picker))}</div>
        <details id="proxy-pool-source-settings" class="proxy-pool-source"${picker.sourceSettingsOpen ? " open" : ""}>
          <summary>代理池来源</summary>
          <div id="proxy-pool-current-source" class="modal-desc proxy-pool-path">${escapeHtml(picker.pool?.sourcePath ?? "自动查找现有代理池")}</div>
          <label class="modal-field-label" for="proxy-pool-discovered-source">已发现的本地文件</label>
          <select id="proxy-pool-discovered-source" class="modal-input">
            <option value="">选择候选文件后点击应用</option>
            ${(picker.pool?.discoveredSources ?? []).map((path) => `<option value="${escapeHtml(path)}"${path === picker.sourceDraft ? " selected" : ""}>${escapeHtml(path)}</option>`).join("")}
          </select>
          <label class="modal-field-label" for="proxy-pool-source-path">或填写绝对 YAML 路径</label>
          <input id="proxy-pool-source-path" class="modal-input" spellcheck="false" placeholder="C:/path/to/sidecar.yaml" value="${escapeHtml(picker.sourceDraft)}" />
          <div class="proxy-pool-source-actions">
            <button id="proxy-pool-source-apply" class="btn" type="button"${picker.loading ? " disabled" : ""}>应用来源</button>
            <button id="proxy-pool-source-auto" class="btn" type="button"${picker.loading ? " disabled" : ""}>自动发现</button>
          </div>
        </details>
        <div class="modal-desc">可用性会实际通过代理请求当前服务页面：返回有效 HTTP 响应的入口置顶，仅能建立代理握手的入口居中，两者都失败的入口置底。只读取现有池，不修改或重启代理服务。</div>
      </div>
    ` : `
      <label class="modal-field-label" for="${manualId}">HTTP / SOCKS5 地址</label>
      <input id="${manualId}" class="modal-input" maxlength="240" spellcheck="false" placeholder="http://127.0.0.1:18081" value="${escapeHtml(manualDraft)}" />
      <div class="modal-desc">手动保存会脱离代理池跟踪。机场订阅先由代理客户端解析，再填写本机固定端口；这里不接收订阅链接或密码。</div>
    `}
  `;
}

function refreshProxyPickerDom(): void {
  const picker = proxyPicker;
  if (!picker || !hasProxyPicker()) return;
  if (picker.mode === "gate") {
    const search = document.querySelector<HTMLInputElement>("#proxy-gate-search");
    if (search && document.activeElement !== search) search.value = picker.gateSearch;
    const select = document.querySelector<HTMLSelectElement>("#proxy-gate-select");
    if (select) {
      const options = proxyGateOptions(picker);
      if (select.dataset.optionsHtml !== options) {
        select.innerHTML = options;
        select.dataset.optionsHtml = options;
      }
      select.value = picker.gateSelectedEntryId ?? "";
      select.disabled =
        picker.gateLoading ||
        picker.gateProbing ||
        Boolean(picker.gateError) ||
        !picker.gate?.entries.length;
    }
    const status = document.querySelector<HTMLElement>("#proxy-gate-status");
    if (status) {
      status.textContent = proxyGateStatus(picker);
      status.classList.toggle(
        "proxy-pool-warning",
        Boolean(picker.gateError || picker.gateProbeError || picker.gate?.error),
      );
    }
    const detail = document.querySelector<HTMLElement>("#proxy-gate-selection-detail");
    if (detail) detail.textContent = proxyGateSelectedDetail(picker);
    const progress = document.querySelector<HTMLElement>("#proxy-gate-probe-progress");
    if (progress) progress.hidden = !picker.gateProbing;
    const currentSource = document.querySelector<HTMLElement>("#proxy-gate-current-source");
    if (currentSource) currentSource.textContent = picker.gate?.poolPath ?? "自动发现现有代理池";
    const sourceInput = document.querySelector<HTMLInputElement>("#proxy-gate-source-path");
    if (sourceInput && document.activeElement !== sourceInput) {
      sourceInput.value = picker.gateSourceDraft;
    }
    const refresh = document.querySelector<HTMLButtonElement>("#proxy-gate-refresh");
    const start = document.querySelector<HTMLButtonElement>("#proxy-gate-start");
    const stop = document.querySelector<HTMLButtonElement>("#proxy-gate-stop");
    const unassign = document.querySelector<HTMLButtonElement>("#proxy-gate-unassign");
    const sourceApply = document.querySelector<HTMLButtonElement>("#proxy-gate-source-apply");
    const sourceAuto = document.querySelector<HTMLButtonElement>("#proxy-gate-source-auto");
    if (refresh) refresh.disabled = picker.gateLoading || picker.gateProbing || picker.gateActionLoading;
    if (start) start.disabled = !picker.gate?.available || Boolean(picker.gate.running) || picker.gateActionLoading;
    if (stop) stop.disabled = !picker.gate?.available || !picker.gate.running || picker.gateActionLoading;
    if (unassign) unassign.disabled = picker.gateActionLoading;
    if (sourceApply) sourceApply.disabled = picker.gateActionLoading;
    if (sourceAuto) sourceAuto.disabled = picker.gateActionLoading;
    const primary = document.querySelector<HTMLButtonElement>("#modal-confirm-btn");
    if (primary) {
      primary.disabled =
        picker.gateLoading ||
        picker.gateProbing ||
        Boolean(picker.gateError) ||
        !picker.gateSelectedEntryId ||
        picker.gateActionLoading;
    }
    return;
  }
  if (picker.mode !== "pool") return;
  const select = document.querySelector<HTMLSelectElement>("#proxy-pool-select");
  if (select) {
    const options = proxyPoolOptions(picker);
    if (select.dataset.optionsHtml !== options) {
      select.innerHTML = options;
      select.dataset.optionsHtml = options;
    }
    select.value = picker.selected?.entryId ?? "";
    select.disabled = (!picker.pool && picker.loading) || Boolean(picker.pool?.error);
  }
  const status = document.querySelector<HTMLElement>("#proxy-pool-status");
  if (status) {
    status.textContent = proxyPoolStatus(picker);
    status.classList.toggle("proxy-pool-warning", Boolean(picker.pool?.error));
  }
  const detail = document.querySelector<HTMLElement>("#proxy-pool-selection-detail");
  if (detail) {
    detail.textContent = proxyPoolSelectedDetail(picker);
    detail.classList.toggle("proxy-pool-warning", Boolean(picker.selectionWarning));
  }
  const current = document.querySelector<HTMLElement>("#proxy-pool-current-source");
  if (current) current.textContent = picker.pool?.sourcePath ?? "自动查找现有代理池";
  const sourceInput = document.querySelector<HTMLInputElement>("#proxy-pool-source-path");
  if (sourceInput && !picker.sourceTouched && document.activeElement !== sourceInput) sourceInput.value = picker.sourceDraft;
  const discovered = document.querySelector<HTMLSelectElement>("#proxy-pool-discovered-source");
  if (discovered) {
    const oldValue = discovered.value;
    const options = `<option value="">选择候选文件后点击应用</option>${(picker.pool?.discoveredSources ?? []).map((path) => `<option value="${escapeHtml(path)}">${escapeHtml(path)}</option>`).join("")}`;
    if (discovered.dataset.optionsHtml !== options) {
      discovered.innerHTML = options;
      discovered.dataset.optionsHtml = options;
      discovered.value = oldValue || picker.sourceDraft;
    }
  }
  for (const id of ["proxy-pool-refresh", "proxy-pool-source-apply", "proxy-pool-source-auto"]) {
    const button = document.querySelector<HTMLButtonElement>(`#${id}`);
    if (button) button.disabled = picker.loading || (id === "proxy-pool-refresh" && picker.healthLoading);
  }
  const primary = document.querySelector<HTMLButtonElement>("#modal-confirm-btn");
  const saveOnly = document.querySelector<HTMLButtonElement>("#modal-save-only-btn");
  if (primary) primary.disabled = picker.loading || Boolean(picker.pool?.error) || !picker.selected || Boolean(picker.selectionWarning);
  if (saveOnly) saveOnly.disabled = picker.loading || Boolean(picker.selectionWarning);
}

async function probeProxyHealth(
  picker: ProxyPickerState,
  revision: string,
  providerId: ProviderId,
  force: boolean,
): Promise<void> {
  if (proxyPicker !== picker || picker.pool?.revision !== revision) return;
  if (!force && picker.healthLoading && picker.healthRevision === revision) return;
  const healthSeq = ++picker.healthSeq;
  picker.healthRevision = revision;
  picker.healthLoading = true;
  picker.healthError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyPoolHealthSnapshot>("probe_proxy_pool", {
      providerId,
      force,
    });
    if (
      proxyPicker !== picker ||
      healthSeq !== picker.healthSeq ||
      !hasProxyPicker() ||
      picker.pool?.revision !== result.revision
    ) {
      return;
    }
    const currentIds = new Set(picker.pool.entries.map((entry) => entry.id));
    picker.health = new Map(
      result.entries
        .filter((entry) => currentIds.has(entry.entryId))
        .map((entry) => [entry.entryId, entry]),
    );
    picker.healthCheckedAtMs = result.checkedAtMs;
  } catch (error) {
    if (proxyPicker !== picker || healthSeq !== picker.healthSeq || !hasProxyPicker()) return;
    picker.healthError = String(error);
  } finally {
    if (proxyPicker === picker && healthSeq === picker.healthSeq) {
      picker.healthLoading = false;
      refreshProxyPickerDom();
    }
  }
}

async function refreshProxyPool(sourcePath?: string, forceHealth = false): Promise<void> {
  const picker = proxyPicker;
  if (!picker || !hasProxyPicker()) return;
  const requestSeq = ++picker.requestSeq;
  const submittedSourceDraft = picker.sourceDraft;
  picker.loading = true;
  refreshProxyPickerDom();
  try {
    const result = sourcePath === undefined
      ? await invoke<ProxyPoolSnapshot>("get_proxy_pool")
      : await invoke<ProxyPoolSnapshot>("set_proxy_pool_source", { sourcePath });
    if (proxyPicker !== picker || requestSeq !== picker.requestSeq || !hasProxyPicker()) return;
    const revisionChanged = picker.pool?.revision !== result.revision;
    picker.pool = { ...result, entries: result.error ? [] : result.entries };
    if (revisionChanged) {
      picker.healthSeq += 1;
      picker.health = new Map();
      picker.healthLoading = false;
      picker.healthError = null;
      picker.healthCheckedAtMs = null;
      picker.healthRevision = null;
    }
    if (!picker.sourceTouched || (sourcePath !== undefined && picker.sourceDraft === submittedSourceDraft)) {
      picker.sourceDraft = result.sourcePath ?? "";
      picker.sourceTouched = false;
    }
    if (picker.selected) {
      const entry = picker.pool.entries.find((item) => item.id === picker.selected!.entryId);
      if (result.error || result.sourcePath !== picker.selected.sourcePath || !entry || entry.fingerprint !== picker.selected.fingerprint) {
        picker.selected = null;
        picker.selectionWarning = "原选择已失效或目标已变化，请从最新列表重新选择确认。";
      } else {
        picker.selected = { ...picker.selected, label: entry.label };
      }
    }
    const providerId = proxyPickerProviderId();
    if (providerId && !result.error && picker.pool.entries.length) {
      void probeProxyHealth(picker, result.revision, providerId, forceHealth);
    }
  } catch (error) {
    if (proxyPicker !== picker || requestSeq !== picker.requestSeq) return;
    if (picker.selected) picker.selectionWarning = "读取失败，原选择已失效；请刷新成功后重新选择。";
    picker.selected = null;
    picker.healthSeq += 1;
    picker.health = new Map();
    picker.healthLoading = false;
    picker.healthError = null;
    picker.healthCheckedAtMs = null;
    picker.healthRevision = null;
    picker.pool = {
      sourcePath: picker.pool?.sourcePath ?? null,
      discoveredSources: picker.pool?.discoveredSources ?? [],
      entries: [], revision: "", checkedAtMs: Date.now(), error: String(error),
    };
  } finally {
    if (proxyPicker === picker && requestSeq === picker.requestSeq) {
      picker.loading = false;
      refreshProxyPickerDom();
    }
  }
}

function applyProxyGateSnapshot(result: ProxyGateSnapshot, picker: ProxyPickerState | null): void {
  proxyGateSnapshot = result;
  proxyGateError = null;
  if (!picker) return;
  picker.gate = result;
  picker.gateError = null;
  picker.gateProbeSeq += 1;
  picker.gateProbing = false;
  picker.gateProbeError = null;
  picker.gateSourceDraft = result.poolPath ?? "";
  if (
    picker.gateSelectedEntryId &&
    !result.entries.some((entry) => entry.entryId === picker.gateSelectedEntryId)
  ) {
    picker.gateSelectedEntryId = null;
  }
  const assignment = proxyGateCurrentAssignment(picker);
  if (!picker.gateSelectedEntryId && assignment) {
    picker.gateSelectedEntryId = assignment.entryId;
  }
}

async function refreshProxyGate(probeEntries = false): Promise<void> {
  const picker = proxyPicker;
  if (!picker || !hasProxyPicker()) return;
  const requestSeq = ++picker.gateRequestSeq;
  picker.gateLoading = true;
  picker.gateError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyGateSnapshot>("get_proxy_gate_snapshot");
    if (
      proxyPicker !== picker ||
      requestSeq !== picker.gateRequestSeq ||
      !hasProxyPicker()
    ) {
      return;
    }
    applyProxyGateSnapshot(result, picker);
    if (probeEntries && result.available && result.entries.length) {
      void probeProxyGatePool(picker);
    }
  } catch (error) {
    if (proxyPicker !== picker || requestSeq !== picker.gateRequestSeq) return;
    proxyGateError = String(error);
    picker.gateError = String(error);
  } finally {
    if (proxyPicker === picker && requestSeq === picker.gateRequestSeq) {
      picker.gateLoading = false;
      refreshProxyPickerDom();
    }
  }
}

function mergeProxyGateProbeEntries(
  entries: ProxyGateEntry[],
  probes: ProxyGateProbeEntry[],
): ProxyGateEntry[] {
  const byId = new Map(probes.map((entry) => [entry.entryId, entry]));
  return entries.map((entry) => {
    const probe = byId.get(entry.entryId);
    if (!probe) return { ...entry, health: null, latencyMs: null, message: null };
    return {
      ...entry,
      displayName: probe.displayName,
      type: probe.type,
      fingerprint: probe.fingerprint,
      chainDepth: probe.chainDepth ?? null,
      health: probe.health,
      latencyMs: probe.latencyMs,
      message: probe.message ?? null,
    };
  });
}

async function probeProxyGatePool(picker: ProxyPickerState): Promise<void> {
  if (!picker.gate || picker.gateProbing || !picker.gate.entries.length) return;
  const probeSeq = ++picker.gateProbeSeq;
  picker.gateProbing = true;
  picker.gateProbeError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyGateProbeSnapshot>("probe_proxy_gate_pool");
    if (
      proxyPicker !== picker ||
      probeSeq !== picker.gateProbeSeq ||
      !picker.gate ||
      !hasProxyPicker()
    ) {
      return;
    }
    picker.gate = {
      ...picker.gate,
      entries: mergeProxyGateProbeEntries(picker.gate.entries, result.entries),
    };
    proxyGateSnapshot = picker.gate;
    picker.gateProbeError = result.error ?? (result.available ? null : "Proxy Gate 探测不可用。");
    if (
      picker.gateSelectedEntryId &&
      !picker.gate.entries.some((entry) => entry.entryId === picker.gateSelectedEntryId)
    ) {
      picker.gateSelectedEntryId = null;
    }
  } catch (error) {
    if (proxyPicker === picker && probeSeq === picker.gateProbeSeq) {
      picker.gateProbeError = String(error);
    }
  } finally {
    if (proxyPicker === picker && probeSeq === picker.gateProbeSeq) {
      picker.gateProbing = false;
      refreshProxyPickerDom();
    }
  }
}

async function refreshProxyGateSummary(): Promise<void> {
  if (snapshot?.platform !== "windows") {
    proxyGateSnapshot = null;
    proxyGateError = null;
    return;
  }
  try {
    const result = await invoke<ProxyGateSnapshot>("get_proxy_gate_snapshot");
    proxyGateSnapshot = result;
    proxyGateError = null;
  } catch (error) {
    proxyGateError = String(error);
  } finally {
    if (snapshot && modal.kind === "none") render();
  }
}

async function runProxyGateAction(
  command: "start_proxy_gate" | "stop_proxy_gate",
): Promise<void> {
  const picker = proxyPicker;
  if (!picker || picker.gateActionLoading) return;
  picker.gateActionLoading = true;
  picker.gateError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyGateSnapshot>(command);
    if (proxyPicker === picker) applyProxyGateSnapshot(result, picker);
  } catch (error) {
    if (proxyPicker === picker) picker.gateError = String(error);
  } finally {
    if (proxyPicker === picker) {
      picker.gateActionLoading = false;
      refreshProxyPickerDom();
    }
  }
}

async function applyProxyGatePoolSource(sourcePath: string): Promise<void> {
  const picker = proxyPicker;
  if (!picker || picker.gateActionLoading) return;
  picker.gateActionLoading = true;
  picker.gateError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyGateSnapshot>("set_proxy_gate_pool_source", {
      sourcePath,
    });
    if (proxyPicker === picker) {
      applyProxyGateSnapshot(result, picker);
      picker.gateSourceDraft = result.poolPath ?? "";
    }
  } catch (error) {
    if (proxyPicker === picker) picker.gateError = String(error);
  } finally {
    if (proxyPicker === picker) {
      picker.gateActionLoading = false;
      refreshProxyPickerDom();
    }
  }
}

async function unassignCurrentProxyGateAccount(): Promise<void> {
  if (modal.kind !== "proxy") return;
  const picker = proxyPicker;
  if (!picker || picker.gateActionLoading) return;
  const account = modal.account;
  picker.gateActionLoading = true;
  picker.gateError = null;
  refreshProxyPickerDom();
  try {
    const result = await invoke<ProxyGateSnapshot>("unassign_proxy_gate_account", {
      providerId: account.providerId,
      uuid: account.uuid,
    });
    applyProxyGateSnapshot(result, picker);
    picker.gateSelectedEntryId = null;
    await setModal({ kind: "none" });
    await callCommand("get_manager_state");
  } catch (error) {
    if (proxyPicker === picker) {
      picker.gateError = String(error);
      if (modal.kind === "proxy") modal.error = String(error);
    }
  } finally {
    if (proxyPicker === picker) {
      picker.gateActionLoading = false;
      refreshProxyPickerDom();
    }
  }
}

function proxySubmission(manualDraft: string, allowEmpty: boolean): {
  proxyUrl: string;
  proxyPoolBinding: ProxyPoolBinding | null;
  error: string | null;
} {
  if (proxyPicker?.mode !== "pool") {
    const proxyUrl = manualDraft.trim();
    return { proxyUrl, proxyPoolBinding: null, error: !allowEmpty && !proxyUrl ? "请先填写独立代理，或仅保存为待配置账号。" : validateProxyDraft(proxyUrl) };
  }
  const picker = proxyPicker;
  if (picker.loading) return { proxyUrl: "", proxyPoolBinding: null, error: "正在同步代理池，请稍后重试。" };
  if (picker.selectionWarning) return { proxyUrl: "", proxyPoolBinding: null, error: picker.selectionWarning };
  const selected = picker.selected;
  if (!selected && allowEmpty) return { proxyUrl: "", proxyPoolBinding: null, error: null };
  const entry = picker.pool?.entries.find((item) => item.id === selected?.entryId);
  if (!selected || !entry || picker.pool?.error || selected.sourcePath !== picker.pool?.sourcePath || selected.fingerprint !== entry.fingerprint) {
    return { proxyUrl: "", proxyPoolBinding: null, error: "请从最新代理池选择一个入口；也可切到手动地址。" };
  }
  return { proxyUrl: entry.proxyUrl, proxyPoolBinding: { ...selected, label: entry.label }, error: null };
}

const updateState: {
  checking: boolean;
  installing: boolean;
  result: UpdateCheckResult | null;
  progress: UpdateProgressEvent | null;
  error: string | null;
  outcomeStatus: string | null;
  outcomeMessage: string | null;
} = {
  checking: false,
  installing: false,
  result: null,
  progress: null,
  error: null,
  outcomeStatus: null,
  outcomeMessage: null,
};

const scrollOffsets: Record<ProviderId, number> = {
  muse: 0,
  hark: 0,
};

function isSameKey(a: AccountKey | null, b: AccountRecord): boolean {
  return Boolean(a && a.providerId === b.providerId && a.uuid === b.uuid);
}

function effectiveSwitchingKey(state: ManagerSnapshot): AccountKey | null {
  return state.switchingAccount ?? optimisticSwitching;
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function validateProxyDraft(value: string): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "socks5:") {
      return "请填写 HTTP 或 SOCKS5 代理端口；机场订阅链接需先导入 Mihomo 等客户端。";
    }
    if (!url.hostname || !url.port || (url.pathname && url.pathname !== "/") || url.search || url.hash) {
      return "代理地址只填写协议、主机和端口，例如 http://127.0.0.1:18081。";
    }
    if (url.username || url.password) {
      return "这里暂不支持代理用户名和密码，请由本地代理客户端处理上游认证。";
    }
    return null;
  } catch {
    return "代理地址格式错误，例如 http://127.0.0.1:18081。";
  }
}

function accountNetworkPresentation(state: ManagerSnapshot, account: AccountRecord): {
  className: string;
  label: string;
  message: string | null;
} {
  const networkState = (state.accountNetworkStates ?? []).find(
    (entry) => entry.providerId === account.providerId && entry.uuid === account.uuid
  );
  if (account.proxyRequired && !account.proxyUrl) {
    return { className: "pending", label: "待配置 · 已阻止打开", message: "设置独立代理后才能打开此账号。" };
  }
  const gateAssignment = proxyGateSnapshot?.assignments.find(
    (assignment) =>
      assignment.providerId === account.providerId && assignment.uuid === account.uuid,
  );
  if (gateAssignment) {
    if (!gateAssignment.bindingValid || gateAssignment.allowed === false) {
      return {
        className: "interrupted",
        label: "Gate 绑定失效 · 已阻止打开",
        message: gateAssignment.error || "Proxy Gate 绑定目标或整链指纹已变化，请重新选择入口。",
      };
    }
    if (proxyGateError) {
      return {
        className: "interrupted",
        label: "Gate 状态不可读 · 已阻止打开",
        message: proxyGateError,
      };
    }
    if (proxyGateSnapshot && (!proxyGateSnapshot.available || !proxyGateSnapshot.running)) {
      return {
        className: "interrupted",
        label: "Gate 未运行 · 已阻止打开",
        message: "启动 Proxy Gate 后才能打开此账号；Gate 不会回退直连。",
      };
    }
    if (gateAssignment.health === "unavailable") {
      return {
        className: "interrupted",
        label: "Gate 上游不可用 · 已阻止打开",
        message: gateAssignment.error || "当前上游代理不可用，打开前还会重新检查。",
      };
    }
  }
  if (networkState?.status === "interrupted") {
    return { className: "interrupted", label: "代理中断 · 已阻止打开", message: networkState.message };
  }
  if (networkState?.status === "checking") {
    return { className: "checking", label: "正在检查代理…", message: null };
  }
  const running = isSameKey(state.activeAccount, account) || state.warmAccounts.some(
    (key) => key.providerId === account.providerId && key.uuid === account.uuid
  );
  if (account.proxyUrl) {
    if (gateAssignment) {
      return {
        className: "gate",
        label:
          gateAssignment.health === "handshake_only"
            ? "Gate 仅握手 · 打开时复查"
            : "Gate 就绪 · 打开时复查",
        message: gateAssignment.error ?? null,
      };
    }
    return {
      className: "configured",
      label: account.pendingActivation
        ? "已配置 · 待打开"
        : running && networkState?.status === "ready"
          ? "已打开 · 预检通过"
          : "已配置 · 打开时检查",
      message: null,
    };
  }
  return { className: "direct", label: "旧账号 · 系统默认网络", message: null };
}

function renderProviderLogo(id: ProviderId, extraClass = ""): string {
  const cfg = PROVIDER_LOGOS[id];
  const cls = `${extraClass} ${cfg.fullBleed ? "full-bleed" : ""}`.trim();
  return `<img src="${cfg.src}" alt="${escapeHtml(id)}" class="${cls}" draggable="false" />`;
}

function computeAvatarText(acc: AccountRecord): string {
  if (acc.autoAlias) {
    return "@";
  }
  const clean = acc.alias.trim();
  const first = Array.from(clean)[0];
  return first ? first.toUpperCase() : "#";
}

function splitQuotaValue(raw: string): { main: string; note: string | null } {
  const m = raw.trim().match(/^(.*?)\s*[（(]([^）)]+)[）)]\s*$/);
  if (m && m[1].trim()) {
    return { main: m[1].trim(), note: m[2].trim() };
  }
  return { main: raw.trim(), note: null };
}

type QuotaStatus = "normal" | "extra" | "low" | "critical";

function resolveQuotaState(acc: AccountRecord): {
  status: QuotaStatus;
  label: string;
  extraDepleted: boolean;
} {
  if (acc.quotaDetail) {
    const d = acc.quotaDetail;
    const planUsed = d.planUsedPercent;
    const planVal = d.planValue || "";
    const planDepleted =
      (planUsed !== null && planUsed >= 100) ||
      planVal.includes("用尽") ||
      planVal.includes("DEPLETED");
    const planLow =
      !planDepleted &&
      ((planUsed !== null && planUsed >= 85) ||
        planVal.includes("偏低") ||
        planVal.includes("LOW"));
    const hasExtra = Boolean(
      d.extraLabel || d.extraValue || d.extraUsedPercent !== null
    );
    const extraUsed = d.extraUsedPercent;
    const extraVal = d.extraValue || "";
    const extraDepleted =
      hasExtra &&
      ((extraUsed !== null && extraUsed >= 100) ||
        extraVal.includes("用尽") ||
        extraVal.includes("耗尽") ||
        extraVal.includes("DEPLETED") ||
        /剩余\s*0(\s|个|口|$)/.test(extraVal));
    const extraLow =
      hasExtra &&
      !extraDepleted &&
      ((extraUsed !== null && extraUsed >= 85) ||
        extraVal.includes("偏低") ||
        extraVal.includes("LOW"));
    const extraIsCap = Boolean(d.extraLabel && /月|monthly/i.test(d.extraLabel));

    if (extraIsCap) {
      if (planDepleted) {
        return { status: "critical", label: "日满", extraDepleted };
      }
      if (extraDepleted) {
        return { status: "critical", label: "月满", extraDepleted };
      }
      if (planLow || extraLow) {
        return { status: "low", label: "偏低", extraDepleted };
      }
      return { status: "normal", label: "", extraDepleted };
    }

    if (planDepleted) {
      if (!hasExtra || extraDepleted) {
        return { status: "critical", label: "用尽", extraDepleted };
      }
      if (extraLow) {
        return { status: "low", label: "偏低", extraDepleted };
      }
      return { status: "extra", label: "用额外", extraDepleted };
    }
    if (planLow) {
      if (!hasExtra || extraDepleted) {
        return { status: "low", label: "偏低", extraDepleted };
      }
      return { status: "normal", label: "", extraDepleted };
    }
    return { status: "normal", label: "", extraDepleted };
  }

  if (acc.quotaPercent === 0 || Boolean(acc.quotaText?.includes("用尽"))) {
    return { status: "critical", label: "用尽", extraDepleted: false };
  }
  if (
    (typeof acc.quotaPercent === "number" && acc.quotaPercent <= 15) ||
    Boolean(acc.quotaText?.includes("偏低"))
  ) {
    return { status: "low", label: "偏低", extraDepleted: false };
  }
  return { status: "normal", label: "", extraDepleted: false };
}

function formatShortResetText(raw: string | null): string {
  const clean = (raw ?? "").trim();
  if (!clean) return "";
  const m = clean.match(/^(?:每周限额将在|周限额重置时间[:：]?|将在)\s*(.+?)\s*$/);
  if (m && m[1]) {
    const inner = m[1].trim();
    return inner.endsWith("重置") ? inner : `${inner}重置`;
  }
  return clean;
}

function formatShortExtraText(
  detail: QuotaDetail,
  extraDepleted: boolean
): string | null {
  if (
    !detail.extraLabel &&
    !detail.extraValue &&
    detail.extraUsedPercent === null
  ) {
    return null;
  }
  const prefix = detail.extraLabel && /月|monthly/i.test(detail.extraLabel) ? "月" : "额外";
  if (extraDepleted) {
    return `${prefix} 100% · 已耗尽`;
  }
  const parsed = splitQuotaValue(detail.extraValue ?? "");
  const pctPart =
    typeof detail.extraUsedPercent === "number"
      ? `${detail.extraUsedPercent}%`
      : parsed.main.replace(/^已使用\s*/, "").trim();
  const notePart = parsed.note
    ? parsed.note
        .replace(/^剩余\s*/, "余 ")
        .replace(/个?口令$/, "")
        .replace(/\s*tokens?$/i, "")
        .trim()
    : "";
  if (pctPart && notePart) {
    return `${prefix} ${pctPart} · ${notePart}`;
  }
  if (notePart) {
    return `${prefix} · ${notePart}`;
  }
  if (pctPart) {
    return `${prefix} ${pctPart}`;
  }
  return detail.extraLabel ?? "额外额度";
}

function renderQuotaDetailPanel(
  detail: QuotaDetail,
  status: QuotaStatus,
  extraDepleted: boolean
): string {
  const shortVal =
    typeof detail.planUsedPercent === "number"
      ? `${detail.planUsedPercent}%`
      : splitQuotaValue(detail.planValue).main;
  const clamped =
    typeof detail.planUsedPercent === "number"
      ? Math.max(0, Math.min(100, detail.planUsedPercent))
      : null;
  const barHtml =
    clamped !== null
      ? `<div class="quota-bar-track ${status}"><div class="quota-bar-fill ${status} ${clamped > 0 ? "has-fill" : ""}" style="width:${clamped}%"></div></div>`
      : `<div class="quota-compact-spacer"></div>`;

  const rawReset = detail.planSubtext ?? "";
  const shortReset = formatShortResetText(rawReset);
  const shortExtra = formatShortExtraText(detail, extraDepleted);
  const fullExtraTitle = [
    detail.extraLabel,
    detail.extraValue,
    detail.extraSubtext,
  ]
    .filter(Boolean)
    .join(" · ");

  const metaHtml =
    shortReset || shortExtra
      ? `
        <div class="quota-compact-meta">
          <span class="quota-sub-text" title="${escapeHtml(rawReset)}">${escapeHtml(shortReset)}</span>
          ${shortExtra ? `<span class="quota-extra-pill ${status}" title="${escapeHtml(fullExtraTitle)}">${escapeHtml(shortExtra)}</span>` : ""}
        </div>
      `
      : "";

  return `
    <div class="quota-compact-main">
      <span class="quota-plan-label ${status}">${escapeHtml(detail.planLabel)}</span>
      ${barHtml}
      <span class="quota-val ${status}">${escapeHtml(shortVal)}</span>
    </div>
    ${metaHtml}
  `;
}

function saveScrollPosition(): void {
  if (!snapshot) return;
  const listEl = document.querySelector<HTMLElement>(".account-list");
  if (listEl) {
    scrollOffsets[snapshot.selectedProviderId] = listEl.scrollTop;
  }
}

function restoreScrollPosition(): void {
  if (!snapshot) return;
  const listEl = document.querySelector<HTMLElement>(".account-list");
  if (listEl) {
    listEl.scrollTop = scrollOffsets[snapshot.selectedProviderId] ?? 0;
  }
}

function rerenderPreservingScroll(): void {
  saveScrollPosition();
  render();
  restoreScrollPosition();
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let idx = 0;
  let val = bytes;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx += 1;
  }
  return idx === 0 ? `${Math.round(val)} B` : `${val.toFixed(1)} ${units[idx]}`;
}

function formatProgressSummary(evt: UpdateProgressEvent): string {
  if (evt.stage === "verifying") return "校验中";
  if (evt.stage === "installing") return "正在替换";
  if (evt.stage === "ready") return "完成";
  if (evt.stage === "needs_permission") return "待授权";
  const pct = typeof evt.percent === "number" ? `${evt.percent}%` : "";
  const transferred = evt.total
    ? `${formatBytes(evt.downloaded)} / ${formatBytes(evt.total)}`
    : evt.downloaded > 0
      ? formatBytes(evt.downloaded)
      : "";
  if (pct && transferred) return `${pct} · ${transferred}`;
  return pct || transferred;
}

async function runCheckForUpdates(silent: boolean): Promise<void> {
  if (updateState.checking || updateState.installing) return;
  updateState.checking = true;
  if (!silent) {
    updateState.error = null;
    updateState.outcomeStatus = null;
    updateState.outcomeMessage = null;
    if (modal.kind === "update") {
      rerenderPreservingScroll();
    }
  }
  try {
    const res = await invoke<UpdateCheckResult>("check_for_updates");
    updateState.result = res;
    updateState.error = null;
  } catch (err) {
    if (!silent) {
      updateState.error = String(err);
    }
  } finally {
    updateState.checking = false;
    if (!silent || updateState.result?.hasUpdate || modal.kind === "update") {
      rerenderPreservingScroll();
    }
  }
}

async function runDownloadAndInstall(): Promise<void> {
  const res = updateState.result;
  if (!res?.downloadUrl || updateState.checking || updateState.installing) return;
  updateState.installing = true;
  updateState.error = null;
  updateState.outcomeStatus = null;
  updateState.outcomeMessage = null;
  lastRenderedUpdateStage = "downloading";
  updateState.progress = {
    stage: "downloading",
    downloaded: 0,
    total: res.size,
    percent: 0,
    message: "正在准备下载...",
  };
  rerenderPreservingScroll();
  try {
    const outcome = await invoke<InstallUpdateOutcome>("download_and_install_update", {
      request: {
        version: res.latestVersion,
        downloadUrl: res.downloadUrl,
        sha256: res.sha256,
        filename: res.filename,
      },
    });
    updateState.outcomeStatus = outcome.status;
    updateState.outcomeMessage = outcome.message;
  } catch (err) {
    updateState.error = String(err);
  } finally {
    updateState.installing = false;
    rerenderPreservingScroll();
  }
}

async function setModal(next: ModalMode): Promise<void> {
  const wasActive = modal.kind !== "none";
  const isActive = next.kind !== "none";
  modal = next;
  if (snapshot?.platform === "windows" && (next.kind === "add" || next.kind === "proxy")) {
    const binding = next.kind === "proxy" ? next.account.proxyPoolBinding ?? null : null;
    const gateAssignment = next.kind === "proxy"
      ? proxyGateSnapshot?.assignments.find(
          (assignment) =>
            assignment.providerId === next.account.providerId &&
            assignment.uuid === next.account.uuid,
        ) ?? null
      : null;
    proxyPicker = {
      mode: gateAssignment ? "gate" : next.kind === "add" || binding ? "pool" : "manual",
      pool: null, selected: binding ? { ...binding } : null, loading: true,
      search: "", sourceDraft: binding?.sourcePath ?? "", sourceTouched: false,
      sourceSettingsOpen: false, selectionWarning: null, requestSeq: 0,
      health: new Map(), healthLoading: false, healthError: null,
      healthCheckedAtMs: null, healthSeq: 0, healthRevision: null,
      gate: proxyGateSnapshot, gateLoading: false, gateError: proxyGateError,
      gateProbing: false, gateProbeError: null, gateProbeSeq: 0,
      gateSearch: "", gateSourceDraft: proxyGateSnapshot?.poolPath ?? "",
      gateSelectedEntryId: gateAssignment?.entryId ?? null,
      gateActionLoading: false, gateRequestSeq: 0,
    };
  } else {
    proxyPicker = null;
  }
  if (wasActive !== isActive) {
    try {
      snapshot = await invoke<ManagerSnapshot>("set_modal_active", {
        active: isActive,
      });
    } catch (err) {
      if (snapshot) {
        snapshot = { ...snapshot, notice: String(err) };
      }
    }
  }
  render();
  if (next.kind === "add" || next.kind === "rename") {
    const input = document.querySelector<HTMLInputElement>("#modal-alias-input");
    if (input) {
      input.focus();
      input.select();
    }
  } else if (next.kind === "remove") {
    document.querySelector<HTMLButtonElement>("#modal-confirm-btn")?.focus();
  } else if (next.kind === "proxy") {
    const selector =
      proxyPicker?.mode === "pool"
        ? "#proxy-pool-search"
        : proxyPicker?.mode === "gate"
          ? "#proxy-gate-search"
          : "#modal-proxy-input";
    const input = document.querySelector<HTMLInputElement>(selector);
    input?.focus();
    input?.select();
  }
  if (hasProxyPicker() && proxyPicker?.mode === "gate") {
    void refreshProxyGate(true);
  } else if (hasProxyPicker()) {
    void refreshProxyPool();
  }
}

async function callCommand(
  cmd: string,
  args: Record<string, unknown> = {},
  optimisticTarget: AccountKey | null = null
): Promise<void> {
  if (busy) return;
  busy = true;
  saveScrollPosition();
  if (optimisticTarget) {
    optimisticSwitching = optimisticTarget;
    render();
    restoreScrollPosition();
  }
  try {
    snapshot = await invoke<ManagerSnapshot>(cmd, args);
  } catch (err) {
    if (snapshot) {
      const currentSnapshot = await invoke<ManagerSnapshot>("get_manager_state").catch(() => snapshot!);
      snapshot = { ...currentSnapshot, notice: String(err) };
    }
  } finally {
    optimisticSwitching = null;
    busy = false;
    render();
    restoreScrollPosition();
  }
}

function currentProvider(state: ManagerSnapshot): ProviderInfo {
  return (
    state.providers.find((p) => p.id === state.selectedProviderId) ??
    state.providers[0]
  );
}

function renderUpdateModal(): string {
  const res = updateState.result;
  const curVer = res ? res.currentVersion : "...";
  const hasUpdate = Boolean(res?.hasUpdate);
  const title = hasUpdate
    ? `发现新版本 v${escapeHtml(res!.latestVersion)}`
    : "检查更新";

  const versionRow = res
    ? `
      <div class="update-version-row">
        <span class="update-version-pill">当前 v${escapeHtml(curVer)}</span>
        ${
          hasUpdate
            ? `<span>→</span><span class="update-version-pill latest">最新 v${escapeHtml(res.latestVersion)}</span>`
            : ""
        }
        ${res.size ? `<span class="update-size-label">${escapeHtml(formatBytes(res.size))}</span>` : ""}
      </div>
    `
    : `<div class="modal-desc">正在连接 GitHub Releases 检查版本信息...</div>`;

  let bodyHtml = "";
  if (updateState.checking && !res) {
    bodyHtml = `
      <div class="update-progress-wrap">
        <div class="update-progress-track"><div class="update-progress-fill indeterminate"></div></div>
      </div>
    `;
  } else if (res) {
    const noteText = res.releaseNotes.trim();
    if (hasUpdate) {
      bodyHtml += `<div class="update-notes">${escapeHtml(noteText || "包含稳定性改进与问题修复。")}</div>`;
      if (!res.downloadUrl) {
        bodyHtml += `<div class="modal-desc">当前发布尚未包含本平台自动安装包，请点击「发布页」手动下载。</div>`;
      }
    } else {
      bodyHtml += `<div class="modal-desc">${escapeHtml(noteText || "当前已是最新版本。")}</div>`;
    }
  }

  const prog = updateState.progress;
  const showProgress =
    prog &&
    prog.stage !== "error" &&
    (updateState.installing ||
      updateState.outcomeStatus === "restarting" ||
      prog.stage === "ready");
  if (showProgress) {
    lastRenderedUpdateStage = prog.stage;
    const isIndeterminate = prog.percent === null;
    const widthStyle = isIndeterminate ? "" : `style="width:${Math.max(0, Math.min(100, prog.percent!))}%"`;
    bodyHtml += `
      <div class="update-progress-wrap">
        <div class="update-progress-meta">
          <span id="update-progress-text">${escapeHtml(prog.message)}</span>
          <span id="update-progress-pct" class="update-progress-pct">${escapeHtml(formatProgressSummary(prog))}</span>
        </div>
        <div class="update-progress-track">
          <div id="update-progress-bar" class="update-progress-fill ${isIndeterminate ? "indeterminate" : ""}" ${widthStyle}></div>
        </div>
      </div>
    `;
  }

  if (updateState.outcomeMessage) {
    bodyHtml += `<div class="update-outcome-box">${escapeHtml(updateState.outcomeMessage)}</div>`;
  }
  if (updateState.error) {
    bodyHtml += `<div class="update-error-box">${escapeHtml(updateState.error)}</div>`;
  }

  const canInstall = hasUpdate && Boolean(res?.downloadUrl);
  const disablePrimary =
    updateState.checking ||
    updateState.installing ||
    updateState.outcomeStatus === "restarting";
  const primaryBtn = canInstall
    ? `<button id="update-install-btn" class="btn primary" type="button" ${disablePrimary ? "disabled" : ""}>${
        updateState.installing
          ? "更新中..."
          : updateState.outcomeStatus === "needs_permission"
            ? "已授权，继续安装"
            : "立即下载并更新"
      }</button>`
    : `<button id="update-recheck-btn" class="btn primary" type="button" ${disablePrimary ? "disabled" : ""}>${
        updateState.checking ? "检查中..." : "重新检查"
      }</button>`;

  const pageBtn = res?.releasePageUrl
    ? `<button id="update-open-page-btn" class="btn" type="button">发布页</button>`
    : "";

  return `
    <div class="inline-modal" role="dialog" aria-modal="true">
      <div class="modal-card">
        <div class="modal-title">${title}</div>
        ${versionRow}
        ${bodyHtml}
        <div class="modal-footer">
          <button id="modal-cancel-btn" class="btn" type="button" ${updateState.installing ? "disabled" : ""}>关闭</button>
          ${pageBtn}
          ${primaryBtn}
        </div>
      </div>
    </div>
  `;
}

function renderModal(provider: ProviderInfo): string {
  if (modal.kind === "none") return "";
  if (modal.kind === "update") {
    return renderUpdateModal();
  }
  if (modal.kind === "add") {
    const supportsAccountProxy = snapshot?.platform === "windows";
    return `
      <div class="inline-modal" role="dialog" aria-modal="true">
        <div class="modal-card">
          <div class="modal-title">新增 ${escapeHtml(provider.shortName)} 账号</div>
          <div class="modal-desc">每个账号拥有独立隔离的登录态。备注可留空，登录成功后将自动显示邮箱名称。</div>
          <label class="modal-field-label" for="modal-alias-input">账号备注</label>
          <input id="modal-alias-input" class="modal-input" maxlength="60" placeholder="备注名称（留空则自动获取登录邮箱）" value="${escapeHtml(modal.draft)}" />
          ${supportsAccountProxy ? `
            ${renderProxyPicker("modal-add-proxy-input", modal.proxyDraft)}
            <div class="modal-desc">新账号必须使用代理。可不选入口“仅保存”为待配置账号；配置并检查通过后才会打开网页。</div>
          ` : ""}
          ${modal.error ? `<div class="modal-error" role="alert">${escapeHtml(modal.error)}</div>` : ""}
          <div class="modal-footer">
            <button id="modal-cancel-btn" class="btn" type="button">取消</button>
            ${supportsAccountProxy ? `<button id="modal-save-only-btn" class="btn" type="button">仅保存</button>` : ""}
            <button id="modal-confirm-btn" class="btn primary" type="button">${supportsAccountProxy ? "检查并打开" : "创建并打开"}</button>
          </div>
        </div>
      </div>
    `;
  }
  if (modal.kind === "rename") {
    return `
      <div class="inline-modal" role="dialog" aria-modal="true">
        <div class="modal-card">
          <div class="modal-title">修改备注名称</div>
          <div class="modal-desc">仅更新本机列表中的显示名称，不影响已登录的官方会话。</div>
          <input id="modal-alias-input" class="modal-input" maxlength="60" placeholder="输入新的备注名称" value="${escapeHtml(modal.draft)}" />
          <div class="modal-footer">
            <button id="modal-cancel-btn" class="btn" type="button">取消</button>
            <button id="modal-confirm-btn" class="btn primary" type="button">保存</button>
          </div>
        </div>
      </div>
    `;
  }
  if (modal.kind === "proxy") {
    return `
      <div class="inline-modal" role="dialog" aria-modal="true">
        <div class="modal-card">
          <div class="modal-title">配置「${escapeHtml(modal.account.alias)}」的独立代理</div>
          ${renderProxyPicker("modal-proxy-input", modal.draft)}
          <div class="modal-desc">${modal.account.proxyRequired || modal.account.proxyUrl ? "此账号必须使用代理。清空后会关闭其会话并阻止打开，直到重新配置代理。" : "旧账号当前使用系统默认网络。保存独立代理后将要求使用代理，之后清空会阻止打开。"}</div>
          <div class="modal-desc">保存后会重建该账号会话并检查连通性。当前未验证出口 IP、DNS / WebRTC 或系统级拒绝直连；配置代理不等于已验证全面网络隔离。</div>
          ${modal.error ? `<div class="modal-error" role="alert">${escapeHtml(modal.error)}</div>` : ""}
          <div class="modal-footer">
            <button id="modal-cancel-btn" class="btn" type="button">取消</button>
            <button id="modal-confirm-btn" class="btn primary" type="button">保存并应用</button>
          </div>
        </div>
      </div>
    `;
  }
  return `
    <div class="inline-modal" role="dialog" aria-modal="true">
      <div class="modal-card">
        <div class="modal-title">移除「${escapeHtml(modal.account.alias)}」</div>
        <div class="modal-desc">将关闭该视图并彻底清除本机独立浏览资料（Cookie 与存储）。云端账号及历史对话仍由官方保留。</div>
        <div class="modal-footer">
          <button id="modal-cancel-btn" class="btn" type="button">取消</button>
          <button id="modal-confirm-btn" class="btn danger" type="button">确认移除</button>
        </div>
      </div>
    </div>
  `;
}

function renderMiddleContent(state: ManagerSnapshot, provider: ProviderInfo): string {
  const providerAccounts = state.accounts.filter((a) => a.providerId === provider.id);
  const switchingKey = effectiveSwitchingKey(state);
  const isAndroid = state.platform === "android";
  const supportsAccountProxy = state.platform === "windows";
  const supportsCookieCopy =
    state.platform === "windows" ||
    state.platform === "macos" ||
    state.platform === "android";

  const noticeHtml = state.notice
    ? `
      <div class="notice-bar">
        <span>${escapeHtml(state.notice)}</span>
        <button id="dismiss-notice-btn" class="notice-close" type="button" title="关闭提示">✕</button>
      </div>
    `
    : "";

  const conflictsHtml = state.conflicts
    .map(
      (c) => `
      <div class="conflict-box">
        <div>检测到 UUID 归属冲突（${escapeHtml(c.uuid.slice(0, 8))}…），请选择保留归属服务：</div>
        <div class="conflict-actions">
          ${c.providers
            .map(
              (pid) =>
                `<button class="mini-btn" data-resolve-uuid="${escapeHtml(c.uuid)}" data-resolve-provider="${escapeHtml(pid)}" type="button">归属 ${escapeHtml(pid.toUpperCase())}</button>`
            )
            .join("")}
        </div>
      </div>
    `
    )
    .join("");

  const hasUpdate = Boolean(updateState.result?.hasUpdate);
  const androidUpdateBtn = isAndroid
    ? `<button id="check-update-btn" class="ghost-icon-btn update-trigger-btn ${hasUpdate ? "has-update" : ""}" type="button" title="${hasUpdate ? `发现新版本 v${escapeHtml(updateState.result!.latestVersion)}` : "检查更新"}">${ICON_UPDATE}${hasUpdate ? `<span class="update-dot"></span>` : ""}</button>`
    : "";

  const headerBtnHtml = provider.enabled
    ? `
      ${androidUpdateBtn}
      ${!isAndroid ? `<button id="sync-all-btn" class="ghost-icon-btn" type="button" title="刷新已就绪账号额度">${ICON_REFRESH}</button>` : ""}
      <button id="open-site-btn" class="ghost-icon-btn" type="button" title="在浏览器中打开 ${escapeHtml(provider.shortName)} 官网">${ICON_EXTERNAL}</button>
      <button id="add-account-btn" class="icon-btn primary" type="button">+ 新增</button>
    `
    : `${androidUpdateBtn}<button id="open-site-btn" class="icon-btn primary" type="button">打开官网 ${ICON_EXTERNAL}</button>`;

  let listBody = "";
  if (!provider.enabled) {
    listBody = `
      <div class="empty-list">
        <div class="empty-icon-circle">${renderProviderLogo(provider.id)}</div>
        <div><strong>${escapeHtml(provider.name)}</strong> 暂未开放内嵌会话</div>
        <div>待核验官方网页直登路径与嵌入条款后启用，可点击右上角访问官网。</div>
      </div>
    `;
  } else if (providerAccounts.length === 0) {
    listBody = `
      <div class="empty-list">
        <div class="empty-icon-circle">${ICON_USER_PLUS}</div>
        <div>暂无 ${escapeHtml(provider.shortName)} 账号</div>
        <div>点击右上角「+ 新增」创建独立登录环境</div>
      </div>
    `;
  } else {
    listBody = providerAccounts
      .map((acc, idx) => {
        const switching = isSameKey(switchingKey, acc);
        const active = !switching && isSameKey(state.activeAccount, acc);
        const avatarText = computeAvatarText(acc);
        const isLoggedOut = acc.authState === "logged_out";
        const hasDetail = !isLoggedOut && Boolean(acc.quotaDetail);

        const quotaState = isLoggedOut
          ? { status: "normal" as QuotaStatus, label: "", extraDepleted: false }
          : resolveQuotaState(acc);
        const {
          status: alertStatus,
          label: alertLabel,
          extraDepleted,
        } = quotaState;

        const isMac = navigator.userAgent.includes("Mac");
        const shortcutHint =
          !isAndroid && idx < 9
            ? `<span class="shortcut-pill" title="快捷切号">${isMac ? "⌘" : "^"}${idx + 1}</span>`
            : "";
        const alertBadgeHtml = isLoggedOut
          ? `<span class="auth-status-badge">未登录</span>`
          : alertStatus !== "normal"
            ? `<span class="quota-warning-badge ${alertStatus}">${alertLabel}</span>`
            : "";

        const sharedProxyPeers = supportsAccountProxy
          ? state.sharedProxyAccounts
              .filter((key) => key.providerId !== acc.providerId || key.uuid !== acc.uuid)
              .map((key) =>
                state.accounts.find(
                  (account) =>
                    account.providerId === key.providerId && account.uuid === key.uuid
                )
              )
              .filter((account): account is AccountRecord => Boolean(account && account.proxyUrl))
          : [];
        const sharedProxyBadge = sharedProxyPeers.length
          ? `<span class="proxy-status shared" title="与账号 ${sharedProxyPeers.map((account) => escapeHtml(account.alias)).join("、")} 共用代理地址">共用代理 · ${sharedProxyPeers.length + 1} 号</span>`
          : "";
        const networkPresentation = supportsAccountProxy ? accountNetworkPresentation(state, acc) : null;

        let compactSubHtml = "";
        if (switching && !hasDetail) {
          compactSubHtml = `<div class="account-sub loading-text"><span class="quota-label-text">正在加载…</span></div>`;
        } else if (isLoggedOut) {
          const loginHint = active
            ? isAndroid
              ? "登录已失效，点击进入重新登录"
              : "登录已失效，请在右侧重新登录"
            : acc.autoAlias
              ? "尚未登录，点击前往登录"
              : "登录已失效，点击重新登录";
          compactSubHtml = `<div class="account-sub auth-logged-out"><span class="auth-dot-inline"></span><span class="quota-label-text">${loginHint}</span></div>`;
        } else if (!hasDetail) {
          const quotaLabel =
            acc.quotaText ?? (acc.autoAlias ? "登录后自动同步额度" : "正在同步额度明细…");
          const isEmpty =
            acc.quotaPercent === 0 || Boolean(acc.quotaText?.includes("用尽"));
          const isLow =
            !isEmpty &&
            ((typeof acc.quotaPercent === "number" && acc.quotaPercent <= 20) ||
              Boolean(acc.quotaText?.includes("偏低")));
          const subClass = isEmpty
            ? "account-sub quota-empty"
            : isLow
              ? "account-sub quota-low"
              : "account-sub";
          const barHtml =
            typeof acc.quotaPercent === "number"
              ? `<span class="mini-quota-track"><span class="mini-quota-fill ${isEmpty ? "empty" : isLow ? "low" : ""}" style="width:${Math.max(0, Math.min(100, acc.quotaPercent))}%"></span></span>`
              : "";
          compactSubHtml = `<div class="${subClass}">${barHtml}<span class="quota-label-text">${escapeHtml(quotaLabel)}</span></div>`;
        }

        const rowClass = [
          "account-row",
          hasDetail ? "has-detail" : "",
          active ? "active" : "",
          switching ? "switching" : "",
          isLoggedOut ? "logged-out" : "",
          alertStatus !== "normal" ? `alert-${alertStatus}` : "",
        ]
          .filter(Boolean)
          .join(" ");

        const avatarClass = [
          "avatar",
          acc.autoAlias ? "pending-identity" : acc.providerId,
        ].join(" ");

        const avatarNodeHtml = acc.avatarUrl
          ? `<div class="avatar has-photo"><img src="${escapeHtml(acc.avatarUrl)}" class="avatar-photo" alt="" draggable="false" /></div>`
          : `<div class="${avatarClass}">${escapeHtml(avatarText)}</div>`;

        return `
          <div class="${rowClass}" data-switch-uuid="${escapeHtml(acc.uuid)}">
            <div class="account-header">
              <div class="avatar-wrap">
                ${avatarNodeHtml}
                ${switching ? `<span class="avatar-spinner"></span>` : ""}
                ${active ? `<span class="status-dot ${isLoggedOut ? "offline" : ""}"></span>` : ""}
              </div>
              <div class="account-meta">
                <div class="account-alias-row">
                  <span class="account-alias">${escapeHtml(acc.alias)}</span>
                  ${shortcutHint}
                  ${alertBadgeHtml}
                  ${sharedProxyBadge}
                  ${acc.proxyPoolBinding ? `<span class="proxy-status pool" title="${escapeHtml(acc.proxyPoolBinding.label)}">池 · ${escapeHtml(acc.proxyPoolBinding.label)}</span>` : ""}
                  ${networkPresentation ? `<span class="proxy-status ${networkPresentation.className}"${networkPresentation.message ? ` title="${escapeHtml(networkPresentation.message)}"` : ""}>${networkPresentation.label}</span>` : ""}
                </div>
                ${compactSubHtml}
                ${networkPresentation?.message ? `<div class="account-network-notice ${networkPresentation.className}">${escapeHtml(networkPresentation.message)}</div>` : ""}
              </div>
              <div class="row-ops">
                ${supportsCookieCopy ? `
                  <button class="action-icon-btn copy-cookie-btn" data-copy-cookie-uuid="${escapeHtml(acc.uuid)}" data-copy-label="复制 Cookie" aria-label="复制 Cookie" type="button">
                    <span class="copy-cookie-icon" aria-hidden="true"></span>
                  </button>
                ` : ""}
                ${state.platform === "windows" && provider.id === "muse" ? `
                  <button class="action-icon-btn" data-account-center-uuid="${escapeHtml(acc.uuid)}" title="打开 Meta 账户管理（同账号代理与隔离环境；再次点击返回 Muse）" aria-label="打开 Meta 账户管理" type="button">${ICON_EXTERNAL}</button>
                ` : ""}
                <button class="action-icon-btn" data-rename-uuid="${escapeHtml(acc.uuid)}" title="修改备注" type="button">${ICON_EDIT}</button>
                ${supportsAccountProxy ? `<button class="action-icon-btn proxy-action ${acc.proxyUrl ? "configured" : ""}" data-proxy-uuid="${escapeHtml(acc.uuid)}" title="${acc.proxyUrl ? "已配置独立代理（切号前检查连通性）" : "配置独立代理"}" type="button">${ICON_NETWORK}</button>` : ""}
                <button class="action-icon-btn danger" data-remove-uuid="${escapeHtml(acc.uuid)}" title="从本机移除" type="button">${ICON_TRASH}</button>
              </div>
            </div>
            ${!isLoggedOut && acc.quotaDetail ? renderQuotaDetailPanel(acc.quotaDetail, alertStatus, extraDepleted) : ""}
          </div>
        `;
      })
      .join("");
  }

  return `
    <div class="column-header">
      <div class="provider-title-wrap">
        <span class="provider-title">${escapeHtml(provider.name)}</span>
        ${
          !provider.enabled
            ? `<span class="status-pill">${escapeHtml(provider.statusText)}</span>`
            : providerAccounts.length > 0
              ? `<span class="count-badge">${providerAccounts.length}</span>`
              : ""
        }
      </div>
      <div class="header-actions">${headerBtnHtml}</div>
    </div>
    ${noticeHtml}
    ${conflictsHtml}
    <div class="account-list">${listBody}</div>
    ${renderModal(provider)}
  `;
}

function renderRightStage(state: ManagerSnapshot, provider: ProviderInfo): string {
  const switchingKey = effectiveSwitchingKey(state);
  const switchingAcc = switchingKey
    ? state.accounts.find(
        (a) => a.providerId === switchingKey.providerId && a.uuid === switchingKey.uuid
      )
    : undefined;

  if (switchingKey) {
    const targetName = switchingAcc?.alias ?? provider.shortName;
    return `
      <main class="right-placeholder">
        <div class="stage-card">
          <div class="stage-logo-orbit">
            <span class="stage-orbit-ring"></span>
            <div class="stage-logo-tile">${renderProviderLogo(provider.id)}</div>
          </div>
          <div class="placeholder-title">正在加载「${escapeHtml(targetName)}」</div>
          <div class="placeholder-desc">首次打开正在渲染 ${escapeHtml(provider.shortName)} 页面，加载完成后再次切换将瞬间直达…</div>
          <div class="progress-track"><div class="progress-bar"></div></div>
        </div>
      </main>
    `;
  }

  if (!provider.enabled) {
    return `
      <main class="right-placeholder">
        <div class="stage-card">
          <div class="stage-logo-tile">${renderProviderLogo(provider.id)}</div>
          <div class="placeholder-title">${escapeHtml(provider.name)} · 待接入</div>
          <div class="placeholder-desc">首版暂未启用应用内嵌会话。访问官网不会覆盖上次已打开的账号记录。</div>
          <button id="stage-open-site-btn" class="btn primary" type="button">打开 ${escapeHtml(provider.shortName)} 官网</button>
        </div>
      </main>
    `;
  }

  return `
    <main class="right-placeholder">
      <div class="stage-card">
        <div class="stage-logo-tile">${renderProviderLogo(provider.id)}</div>
        <div class="placeholder-title">${escapeHtml(provider.name)}</div>
        <div class="placeholder-desc">从左侧选择一个账号进入对话，或新建账号开启独立隔离登录。</div>
        <button id="stage-add-btn" class="btn primary" type="button">+ 新增 ${escapeHtml(provider.shortName)} 账号</button>
      </div>
    </main>
  `;
}

function render(): void {
  const root = document.querySelector<HTMLElement>("#app");
  if (!root || !snapshot) return;

  const focused = document.activeElement;
  const preserveFocus = focused instanceof HTMLElement && focused.id && focused.closest(".inline-modal")
    ? { id: focused.id, start: focused instanceof HTMLInputElement ? focused.selectionStart : null, end: focused instanceof HTMLInputElement ? focused.selectionEnd : null }
    : null;
  const modalScroll = document.querySelector<HTMLElement>(".inline-modal")?.scrollTop ?? 0;

  const provider = currentProvider(snapshot);
  const isAndroid = snapshot.platform === "android";

  if (isAndroid) {
    root.innerHTML = `
      <div class="mobile-shell">
        <div class="mobile-tabs">
          ${snapshot.providers
            .map(
              (p) => `
              <button class="mobile-tab ${p.id === provider.id ? "active" : ""}" data-provider="${escapeHtml(p.id)}" type="button">
                ${renderProviderLogo(p.id, "mobile-tab-logo")}
                <span>${escapeHtml(p.shortName)}</span>
              </button>
            `
            )
            .join("")}
        </div>
        <div class="middle-column" style="width:100%;min-width:0;flex:1;border-right:none;">
          ${renderMiddleContent(snapshot, provider)}
        </div>
      </div>
    `;
  } else {
    root.innerHTML = `
      <div class="desktop-shell">
        <aside class="service-rail">
          <div class="rail-brand" title="叠号 · AI 助手账号切换器">
            <img src="/logos/app-logo.svg" alt="叠号" class="rail-brand-img" draggable="false" />
          </div>
          <div class="rail-divider"></div>
          ${snapshot.providers
            .map(
              (p) => `
              <button class="rail-btn ${p.id === provider.id ? "active" : ""} ${!p.enabled ? "disabled" : ""}" data-provider="${escapeHtml(p.id)}" title="${escapeHtml(p.name)}" type="button">
                <div class="rail-logo-box">${renderProviderLogo(p.id, "rail-logo-img")}</div>
                <span class="rail-label">${escapeHtml(p.shortName)}</span>
                ${!p.enabled ? `<span class="rail-pending-tag">待接入</span>` : ""}
              </button>
            `
            )
            .join("")}
          <div class="rail-footer">
            <button
              id="check-update-btn"
              class="rail-util-btn ${updateState.result?.hasUpdate ? "has-update" : ""}"
              title="${updateState.result?.hasUpdate ? `发现新版本 v${escapeHtml(updateState.result.latestVersion)}` : "检查更新"}"
              type="button"
            >
              ${ICON_UPDATE}
              <span class="rail-util-label">更新</span>
              ${updateState.result?.hasUpdate ? `<span class="update-dot"></span>` : ""}
            </button>
          </div>
        </aside>
        <section class="middle-column">
          ${renderMiddleContent(snapshot, provider)}
        </section>
        ${renderRightStage(snapshot, provider)}
      </div>
    `;
  }

  bindEvents(snapshot, provider);
  refreshProxyPickerDom();
  const modalElement = document.querySelector<HTMLElement>(".inline-modal");
  if (modalElement) modalElement.scrollTop = modalScroll;
  if (preserveFocus) {
    const replacement = document.getElementById(preserveFocus.id);
    replacement?.focus({ preventScroll: true });
    if (replacement instanceof HTMLInputElement && preserveFocus.start !== null && preserveFocus.end !== null) {
      replacement.setSelectionRange(preserveFocus.start, preserveFocus.end);
    }
  }
}

function bindEvents(state: ManagerSnapshot, provider: ProviderInfo): void {
  document.querySelectorAll<HTMLButtonElement>("[data-proxy-mode]").forEach((button) => {
    button.addEventListener("click", () => {
      if (!proxyPicker || modalSubmitting || busy) return;
      const requestedMode = button.dataset.proxyMode;
      if (requestedMode === "gate" && modal.kind !== "proxy") return;
      proxyPicker.mode =
        requestedMode === "gate" ? "gate" : requestedMode === "manual" ? "manual" : "pool";
      if (modal.kind === "add" || modal.kind === "proxy") modal.error = null;
      render();
      const id =
        proxyPicker.mode === "pool"
          ? "proxy-pool-search"
          : proxyPicker.mode === "gate"
            ? "proxy-gate-search"
            : modal.kind === "add"
              ? "modal-add-proxy-input"
              : "modal-proxy-input";
      document.getElementById(id)?.focus();
      if (proxyPicker.mode === "pool") void refreshProxyPool();
      if (proxyPicker.mode === "gate") void refreshProxyGate(true);
    });
  });
  document.querySelector<HTMLInputElement>("#proxy-pool-search")?.addEventListener("input", (event) => {
    if (!proxyPicker) return;
    proxyPicker.search = (event.currentTarget as HTMLInputElement).value;
    refreshProxyPickerDom();
  });
  document.querySelector<HTMLSelectElement>("#proxy-pool-select")?.addEventListener("change", (event) => {
    const picker = proxyPicker;
    if (!picker || picker.pool?.error) return;
    const entry = picker.pool?.entries.find((item) => item.id === (event.currentTarget as HTMLSelectElement).value);
    picker.selected = entry && picker.pool?.sourcePath ? {
      sourcePath: picker.pool.sourcePath, entryId: entry.id, fingerprint: entry.fingerprint, label: entry.label,
    } : null;
    picker.selectionWarning = null;
    if (modal.kind === "add" || modal.kind === "proxy") modal.error = null;
    document.querySelector(".modal-error")?.remove();
    refreshProxyPickerDom();
  });
  document.querySelector("#proxy-pool-refresh")?.addEventListener("click", () => void refreshProxyPool(undefined, true));
  document.querySelector<HTMLDetailsElement>("#proxy-pool-source-settings")?.addEventListener("toggle", (event) => {
    if (proxyPicker) proxyPicker.sourceSettingsOpen = (event.currentTarget as HTMLDetailsElement).open;
  });
  document.querySelector<HTMLInputElement>("#proxy-pool-source-path")?.addEventListener("input", (event) => {
    if (!proxyPicker) return;
    proxyPicker.sourceDraft = (event.currentTarget as HTMLInputElement).value;
    proxyPicker.sourceTouched = true;
  });
  document.querySelector<HTMLSelectElement>("#proxy-pool-discovered-source")?.addEventListener("change", (event) => {
    if (!proxyPicker) return;
    const value = (event.currentTarget as HTMLSelectElement).value;
    if (!value) return;
    proxyPicker.sourceDraft = value;
    proxyPicker.sourceTouched = true;
    const input = document.querySelector<HTMLInputElement>("#proxy-pool-source-path");
    if (input) input.value = value;
  });
  document.querySelector("#proxy-pool-source-apply")?.addEventListener("click", () => {
    if (proxyPicker) void refreshProxyPool(proxyPicker.sourceDraft.trim());
  });
  document.querySelector("#proxy-pool-source-auto")?.addEventListener("click", () => void refreshProxyPool(""));
  document.querySelector<HTMLInputElement>("#proxy-gate-search")?.addEventListener("input", (event) => {
    if (!proxyPicker) return;
    proxyPicker.gateSearch = (event.currentTarget as HTMLInputElement).value;
    refreshProxyPickerDom();
  });
  document.querySelector<HTMLInputElement>("#proxy-gate-search")?.addEventListener("keydown", async (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      document.querySelector<HTMLButtonElement>("#modal-confirm-btn")?.click();
    } else if (event.key === "Escape") {
      event.preventDefault();
      await setModal({ kind: "none" });
    }
  });
  document.querySelector<HTMLSelectElement>("#proxy-gate-select")?.addEventListener("change", (event) => {
    if (!proxyPicker) return;
    const value = (event.currentTarget as HTMLSelectElement).value;
    proxyPicker.gateSelectedEntryId =
      proxyPicker.gate?.entries.some((entry) => entry.entryId === value) ? value : null;
    if (modal.kind === "proxy") modal.error = null;
    refreshProxyPickerDom();
  });
  document.querySelector("#proxy-gate-refresh")?.addEventListener("click", () => void refreshProxyGate(true));
  document.querySelector("#proxy-gate-start")?.addEventListener("click", () => {
    void runProxyGateAction("start_proxy_gate");
  });
  document.querySelector("#proxy-gate-stop")?.addEventListener("click", () => {
    void runProxyGateAction("stop_proxy_gate");
  });
  document.querySelector("#proxy-gate-unassign")?.addEventListener("click", () => {
    void unassignCurrentProxyGateAccount();
  });
  document.querySelector<HTMLInputElement>("#proxy-gate-source-path")?.addEventListener("input", (event) => {
    if (!proxyPicker) return;
    proxyPicker.gateSourceDraft = (event.currentTarget as HTMLInputElement).value;
  });
  document.querySelector("#proxy-gate-source-apply")?.addEventListener("click", () => {
    if (proxyPicker) void applyProxyGatePoolSource(proxyPicker.gateSourceDraft.trim());
  });
  document.querySelector("#proxy-gate-source-auto")?.addEventListener("click", () => {
    if (!proxyPicker) return;
    proxyPicker.gateSourceDraft = "";
    const input = document.querySelector<HTMLInputElement>("#proxy-gate-source-path");
    if (input) input.value = "";
    void applyProxyGatePoolSource("");
  });
  document.querySelectorAll<HTMLElement>("[data-provider]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const nextId = btn.dataset.provider as ProviderId;
      if (!nextId || nextId === state.selectedProviderId) return;
      if (modal.kind !== "none") {
        await setModal({ kind: "none" });
      }
      await callCommand("select_provider", { providerId: nextId });
    });
  });

  const openOfficial = async () => {
    await callCommand("open_official_site", { providerId: provider.id });
  };
  document.querySelector("#open-site-btn")?.addEventListener("click", openOfficial);
  document.querySelector("#stage-open-site-btn")?.addEventListener("click", openOfficial);

  document.querySelector("#sync-all-btn")?.addEventListener("click", async () => {
    const btn = document.querySelector("#sync-all-btn");
    btn?.classList.add("spinning");
    try {
      await callCommand("trigger_quota_sync");
    } finally {
      setTimeout(() => btn?.classList.remove("spinning"), 650);
    }
  });

  const openAddModal = async () => {
    await setModal({ kind: "add", providerId: provider.id, draft: "", proxyDraft: "", error: null });
  };
  document.querySelector("#add-account-btn")?.addEventListener("click", openAddModal);
  document.querySelector("#stage-add-btn")?.addEventListener("click", openAddModal);

  document.querySelector("#dismiss-notice-btn")?.addEventListener("click", async () => {
    await callCommand("dismiss_notice");
  });

  document.querySelector("#check-update-btn")?.addEventListener("click", async () => {
    if (modal.kind !== "none") return;
    await setModal({ kind: "update" });
    if (!updateState.result && !updateState.checking && !updateState.installing) {
      await runCheckForUpdates(false);
    }
  });

  document.querySelector("#update-recheck-btn")?.addEventListener("click", async () => {
    await runCheckForUpdates(false);
  });

  document.querySelector("#update-install-btn")?.addEventListener("click", async () => {
    await runDownloadAndInstall();
  });

  document.querySelector("#update-open-page-btn")?.addEventListener("click", async () => {
    const pageUrl = updateState.result?.releasePageUrl;
    if (!pageUrl) return;
    try {
      await invoke("open_external_url", { url: pageUrl });
    } catch (err) {
      updateState.error = String(err);
      rerenderPreservingScroll();
    }
  });

  document.querySelectorAll<HTMLElement>("[data-resolve-uuid]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const uuid = btn.dataset.resolveUuid;
      const keepProviderId = btn.dataset.resolveProvider;
      if (uuid && keepProviderId) {
        await callCommand("resolve_uuid_conflict", { keepProviderId, uuid });
      }
    });
  });

  document.querySelectorAll<HTMLButtonElement>("[data-copy-cookie-uuid]").forEach((button) => {
    let feedbackTimer: number | undefined;
    button.addEventListener("click", async (event) => {
      event.stopPropagation();
      const uuid = button.dataset.copyCookieUuid;
      if (!uuid) return;
      const copyKey = `${provider.id}:${uuid}`;
      if (copyingCookies.has(copyKey)) return;
      copyingCookies.add(copyKey);
      window.clearTimeout(feedbackTimer);
      button.classList.remove("copied");
      button.dataset.copyLabel = "复制中…";
      try {
        await invoke<void>("copy_account_cookie", { providerId: provider.id, uuid });
        button.dataset.copyLabel = "已复制 Cookie";
        button.classList.add("copied");
        feedbackTimer = window.setTimeout(() => {
          button.classList.remove("copied");
          button.dataset.copyLabel = "复制 Cookie";
        }, 1800);
      } catch (err) {
        button.dataset.copyLabel = "复制 Cookie";
        if (snapshot?.selectedProviderId === provider.id) {
          snapshot = { ...snapshot, notice: String(err) };
          rerenderPreservingScroll();
        }
      } finally {
        copyingCookies.delete(copyKey);
      }
    });
  });

  document.querySelectorAll<HTMLElement>("[data-rename-uuid]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const uuid = btn.dataset.renameUuid;
      const acc = state.accounts.find((a) => a.providerId === provider.id && a.uuid === uuid);
      if (acc) {
        await setModal({
          kind: "rename",
          account: acc,
          draft: acc.autoAlias ? "" : acc.alias,
        });
      }
    });
  });

  document.querySelectorAll<HTMLElement>("[data-account-center-uuid]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const uuid = btn.dataset.accountCenterUuid;
      if (!uuid) return;
      const alreadyWarm = state.warmAccounts.some(
        (key) => key.providerId === provider.id && key.uuid === uuid
      );
      const alreadyActive =
        state.activeAccount?.providerId === provider.id &&
        state.activeAccount?.uuid === uuid;
      await callCommand(
        "toggle_muse_account_management",
        { providerId: provider.id, uuid },
        !alreadyActive && !alreadyWarm ? { providerId: provider.id, uuid } : null
      );
    });
  });

  document.querySelectorAll<HTMLElement>("[data-proxy-uuid]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const uuid = btn.dataset.proxyUuid;
      const acc = state.accounts.find((a) => a.providerId === provider.id && a.uuid === uuid);
      if (acc) {
        await setModal({ kind: "proxy", account: acc, draft: acc.proxyUrl ?? "", error: null });
      }
    });
  });

  document.querySelectorAll<HTMLElement>("[data-remove-uuid]").forEach((btn) => {
    btn.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      const uuid = btn.dataset.removeUuid;
      const acc = state.accounts.find((a) => a.providerId === provider.id && a.uuid === uuid);
      if (acc) {
        await setModal({ kind: "remove", account: acc });
      }
    });
  });

  document.querySelectorAll<HTMLElement>("[data-switch-uuid]").forEach((row) => {
    row.addEventListener("click", async () => {
      if (modal.kind !== "none" || state.conflicts.length > 0) return;
      const uuid = row.dataset.switchUuid;
      if (!uuid) return;
      if (
        state.activeAccount?.providerId === provider.id &&
        state.activeAccount?.uuid === uuid &&
        state.platform !== "android"
      ) {
        return;
      }
      const alreadyWarm = state.warmAccounts.some(
        (k) => k.providerId === provider.id && k.uuid === uuid
      );
      await callCommand(
        "switch_account",
        { providerId: provider.id, uuid },
        alreadyWarm ? null : { providerId: provider.id, uuid }
      );
    });
  });

  const aliasInput = document.querySelector<HTMLInputElement>("#modal-alias-input");
  aliasInput?.addEventListener("input", () => {
    if (modal.kind === "add" || modal.kind === "rename") {
      modal.draft = aliasInput.value;
    }
  });
  aliasInput?.addEventListener("keydown", async (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      document.querySelector<HTMLButtonElement>("#modal-confirm-btn")?.click();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      await setModal({ kind: "none" });
    }
  });

  const proxyInput = document.querySelector<HTMLInputElement>("#modal-proxy-input");
  proxyInput?.addEventListener("input", () => {
    if (modal.kind === "proxy") modal.draft = proxyInput.value;
  });

  const addProxyInput = document.querySelector<HTMLInputElement>("#modal-add-proxy-input");
  addProxyInput?.addEventListener("input", () => {
    if (modal.kind === "add") modal.proxyDraft = addProxyInput.value;
  });
  addProxyInput?.addEventListener("keydown", async (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      document.querySelector<HTMLButtonElement>("#modal-confirm-btn")?.click();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      await setModal({ kind: "none" });
    }
  });

  const submitAdd = async (openAfterSave: boolean) => {
    if (busy || modalSubmitting || modal.kind !== "add") return;
    const currentModal = modal;
    const submission = state.platform === "windows"
      ? proxySubmission(currentModal.proxyDraft, !openAfterSave)
      : { proxyUrl: "", proxyPoolBinding: null, error: null };
    if (state.platform === "windows") {
      currentModal.error = submission.error;
      if (currentModal.error) {
        render();
        document.getElementById(proxyPicker?.mode === "pool" ? "proxy-pool-select" : "modal-add-proxy-input")?.focus();
        return;
      }
    }
    modalSubmitting = true;
    try {
      await setModal({ kind: "none" });
      await callCommand("add_account", {
        providerId: currentModal.providerId,
        alias: currentModal.draft.trim(),
        proxyUrl: submission.proxyUrl,
        proxyPoolBinding: submission.proxyPoolBinding,
        openAfterSave,
      });
    } finally {
      modalSubmitting = false;
    }
  };
  document.querySelector("#modal-save-only-btn")?.addEventListener("click", async () => {
    await submitAdd(false);
  });
  proxyInput?.addEventListener("keydown", async (ev) => {
    if (ev.key === "Enter") {
      ev.preventDefault();
      document.querySelector<HTMLButtonElement>("#modal-confirm-btn")?.click();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      await setModal({ kind: "none" });
    }
  });

  document.querySelector("#modal-cancel-btn")?.addEventListener("click", async () => {
    if (modal.kind === "update" && updateState.installing) return;
    await setModal({ kind: "none" });
  });

  document.querySelector("#modal-confirm-btn")?.addEventListener("click", async () => {
    if (busy || modalSubmitting) return;
    const currentModal = modal;
    if (currentModal.kind === "add") {
      await submitAdd(true);
    } else if (currentModal.kind === "rename") {
      const alias = currentModal.draft.trim();
      if (!alias) return;
      await setModal({ kind: "none" });
      await callCommand("rename_account", {
        providerId: currentModal.account.providerId,
        uuid: currentModal.account.uuid,
        alias,
      });
    } else if (currentModal.kind === "proxy") {
      if (proxyPicker?.mode === "gate") {
        const picker = proxyPicker;
        const entryId = picker.gateSelectedEntryId;
        const entry = picker.gate?.entries.find((item) => item.entryId === entryId);
        if (!entryId || !entry) {
          currentModal.error = "请从最新 Proxy Gate 入口中选择一项。";
          render();
          document.querySelector<HTMLSelectElement>("#proxy-gate-select")?.focus();
          return;
        }
        modalSubmitting = true;
        picker.gateActionLoading = true;
        currentModal.error = null;
        refreshProxyPickerDom();
        try {
          const result = await invoke<ProxyGateSnapshot>("assign_proxy_gate_account", {
            providerId: currentModal.account.providerId,
            uuid: currentModal.account.uuid,
            accountAlias: currentModal.account.alias,
            entryId,
          });
          applyProxyGateSnapshot(result, picker);
          await setModal({ kind: "none" });
          await callCommand("get_manager_state");
        } catch (error) {
          currentModal.error = String(error);
          if (proxyPicker === picker) {
            picker.gateError = String(error);
            picker.gateActionLoading = false;
            refreshProxyPickerDom();
          }
          if (modal.kind === "proxy") render();
        } finally {
          modalSubmitting = false;
          if (proxyPicker === picker) picker.gateActionLoading = false;
        }
        return;
      }
      const picker = proxyPicker;
      const submission = proxySubmission(currentModal.draft, proxyPicker?.mode === "manual");
      currentModal.error = submission.error;
      if (currentModal.error) {
        render();
        document
          .querySelector<HTMLInputElement | HTMLSelectElement>(
            proxyPicker?.mode === "pool" ? "#proxy-pool-select" : "#modal-proxy-input",
          )
          ?.focus();
        return;
      }
      const oldGateAssignment = picker ? proxyGateCurrentAssignment(picker) : null;
      if (picker && oldGateAssignment) {
        modalSubmitting = true;
        picker.gateActionLoading = true;
        currentModal.error = null;
        refreshProxyPickerDom();
        try {
          const result = await invoke<ProxyGateSnapshot>("unassign_proxy_gate_account", {
            providerId: currentModal.account.providerId,
            uuid: currentModal.account.uuid,
          });
          applyProxyGateSnapshot(result, picker);
        } catch (error) {
          currentModal.error = `解除旧 Proxy Gate 绑定失败：${String(error)}`;
          render();
          return;
        } finally {
          modalSubmitting = false;
          if (proxyPicker === picker) {
            picker.gateActionLoading = false;
            refreshProxyPickerDom();
          }
        }
      }
      await setModal({ kind: "none" });
      await callCommand("set_account_proxy", {
        providerId: currentModal.account.providerId,
        uuid: currentModal.account.uuid,
        proxyUrl: submission.proxyUrl,
        proxyPoolBinding: submission.proxyPoolBinding,
      });
    } else if (currentModal.kind === "remove") {
      await setModal({ kind: "none" });
      await callCommand("remove_account", {
        providerId: currentModal.account.providerId,
        uuid: currentModal.account.uuid,
      });
    }
  });
}

async function bootstrap(): Promise<void> {
  window.addEventListener("focus", () => {
    if (!hasProxyPicker() || proxyPicker?.loading) return;
    if (proxyPicker?.mode === "gate") {
      if (!proxyPicker.gateLoading && !proxyPicker.gateProbing) void refreshProxyGate(true);
    } else {
      void refreshProxyPool();
    }
  });
  window.setInterval(() => {
    if (!hasProxyPicker() || proxyPicker?.loading) return;
    if (proxyPicker?.mode === "gate") {
      if (!proxyPicker.gateLoading && !proxyPicker.gateActionLoading) void refreshProxyGate();
    } else {
      void refreshProxyPool();
    }
  }, 10_000);
  await listen<ManagerSnapshot>("manager-state-changed", (event) => {
    saveScrollPosition();
    snapshot = event.payload;
    if (snapshot.activeAccount || !snapshot.switchingAccount) {
      optimisticSwitching = null;
    }
    render();
    restoreScrollPosition();
    void refreshProxyGateSummary();
  });

  await listen<UpdateProgressEvent>("update-progress", (event) => {
    const evt = event.payload;
    updateState.progress = evt;
    if (evt.stage === "error") {
      updateState.error = evt.message;
    }
    if (modal.kind !== "update") {
      return;
    }
    const barEl = document.querySelector<HTMLElement>("#update-progress-bar");
    const textEl = document.querySelector<HTMLElement>("#update-progress-text");
    const pctEl = document.querySelector<HTMLElement>("#update-progress-pct");
    if (barEl && textEl && pctEl && lastRenderedUpdateStage === evt.stage && evt.stage !== "error") {
      textEl.textContent = evt.message;
      pctEl.textContent = formatProgressSummary(evt);
      if (evt.percent === null) {
        barEl.classList.add("indeterminate");
        barEl.style.width = "";
      } else {
        barEl.classList.remove("indeterminate");
        barEl.style.width = `${Math.max(0, Math.min(100, evt.percent))}%`;
      }
      return;
    }
    lastRenderedUpdateStage = evt.stage;
    rerenderPreservingScroll();
  });

  window.addEventListener("keydown", async (ev) => {
    if (modal.kind !== "none") {
      if (
        ev.key === "Escape" &&
        modal.kind !== "add" &&
        modal.kind !== "rename" &&
        !(modal.kind === "update" && updateState.installing)
      ) {
        ev.preventDefault();
        await setModal({ kind: "none" });
      }
      return;
    }
    const target = ev.target as HTMLElement | null;
    if (
      target &&
      (target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable)
    ) {
      return;
    }
    if ((ev.metaKey || ev.ctrlKey) && !ev.shiftKey && !ev.altKey) {
      const digit = parseInt(ev.key, 10);
      if (!isNaN(digit) && digit >= 1 && digit <= 9) {
        ev.preventDefault();
        if (!snapshot) return;
        const providerAccounts = snapshot.accounts.filter(
          (a) => a.providerId === snapshot!.selectedProviderId
        );
        const targetAcc = providerAccounts[digit - 1];
        if (targetAcc && !isSameKey(snapshot.activeAccount, targetAcc)) {
          const alreadyWarm = snapshot.warmAccounts.some(
            (k) =>
              k.providerId === targetAcc.providerId && k.uuid === targetAcc.uuid
          );
          await callCommand(
            "switch_account",
            { providerId: targetAcc.providerId, uuid: targetAcc.uuid },
            alreadyWarm ? null : { providerId: targetAcc.providerId, uuid: targetAcc.uuid }
          );
        }
        return;
      }
      if (
        ev.key === "ArrowUp" ||
        ev.key === "ArrowDown" ||
        ev.key === "[" ||
        ev.key === "]"
      ) {
        ev.preventDefault();
        if (!snapshot) return;
        const providerAccounts = snapshot.accounts.filter(
          (a) => a.providerId === snapshot!.selectedProviderId
        );
        if (providerAccounts.length <= 1) return;
        const curIdx = providerAccounts.findIndex((a) =>
          isSameKey(snapshot!.activeAccount, a)
        );
        const delta = ev.key === "ArrowUp" || ev.key === "[" ? -1 : 1;
        const nextIdx =
          (curIdx + delta + providerAccounts.length) % providerAccounts.length;
        const targetAcc = providerAccounts[nextIdx];
        if (targetAcc && !isSameKey(snapshot.activeAccount, targetAcc)) {
          const alreadyWarm = snapshot.warmAccounts.some(
            (k) =>
              k.providerId === targetAcc.providerId && k.uuid === targetAcc.uuid
          );
          await callCommand(
            "switch_account",
            { providerId: targetAcc.providerId, uuid: targetAcc.uuid },
            alreadyWarm ? null : { providerId: targetAcc.providerId, uuid: targetAcc.uuid }
          );
        }
      }
    }
  });

  snapshot = await invoke<ManagerSnapshot>("get_manager_state");
  render();
  void refreshProxyGateSummary();
  setTimeout(() => {
    void runCheckForUpdates(true);
  }, 3500);
}

void bootstrap();
