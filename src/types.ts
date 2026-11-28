export type ProviderId = "muse" | "hark";
export type AuthState = "unknown" | "authenticated" | "logged_out";

export interface AccountKey {
  providerId: ProviderId;
  uuid: string;
}

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  shortName: string;
  enabled: boolean;
  statusText: string;
  entryUrl: string;
  homepageUrl: string;
  callbackOrigins: string[];
  verifiedMethods: string[];
}

export interface QuotaDetail {
  planLabel: string;
  planValue: string;
  planSubtext: string | null;
  planUsedPercent: number | null;
  extraLabel: string | null;
  extraValue: string | null;
  extraSubtext: string | null;
  extraUsedPercent: number | null;
}

export interface AccountRecord {
  providerId: ProviderId;
  uuid: string;
  alias: string;
  createdAtMs: number;
  autoAlias: boolean;
  avatarUrl: string | null;
  quotaText: string | null;
  quotaPercent: number | null;
  quotaDetail: QuotaDetail | null;
  authState: AuthState;
  proxyUrl: string | null;
  proxyRequired: boolean;
  pendingActivation: boolean;
  proxyPoolBinding: ProxyPoolBinding | null;
}

export interface ProxyPoolBinding {
  sourcePath: string;
  entryId: string;
  fingerprint: string;
  label: string;
}

export interface ProxyPoolEntry {
  id: string;
  label: string;
  proxyUrl: string;
  fingerprint: string;
  duplicateCount: number;
}

export interface ProxyPoolSnapshot {
  sourcePath: string | null;
  discoveredSources: string[];
  entries: ProxyPoolEntry[];
  revision: string;
  checkedAtMs: number;
  error: string | null;
}

export interface ProxyPoolHealthEntry {
  entryId: string;
  available: boolean;
  handshakeOnly: boolean;
  latencyMs: number;
  message: string | null;
}

export interface ProxyPoolHealthSnapshot {
  providerId: ProviderId;
  revision: string;
  checkedAtMs: number;
  entries: ProxyPoolHealthEntry[];
}

export interface ProxyGateEntry {
  entryId: string;
  displayName: string;
  type: string;
  fingerprint: string;
  chainDepth?: number | null;
  health?: string | null;
  latencyMs?: number | null;
  message?: string | null;
}

export interface ProxyGateAssignment {
  accountId: string;
  providerId: ProviderId;
  uuid: string;
  accountAlias?: string | null;
  entryId: string;
  displayName: string;
  fingerprint: string;
  port: number;
  endpoint: string;
  bindingValid: boolean;
  allowed?: boolean | null;
  health?: string | null;
  latencyMs?: number | null;
  error?: string | null;
}

export interface ProxyGateRuntimeAssignment {
  accountId: string;
  entryId: string;
  fingerprint: string;
  label: string;
  endpoint: string;
  upstreamProxyUrl?: string | null;
  allowed: boolean;
  bindingValid: boolean;
  health?: string | null;
  latencyMs?: number | null;
  error?: string | null;
}

export interface ProxyGateStatus {
  updatedAtMs: number;
  poolPath?: string | null;
  poolRevision?: string | null;
  error?: string | null;
  assignments: ProxyGateRuntimeAssignment[];
}

export interface ProxyGateSnapshot {
  available: boolean;
  running: boolean;
  error?: string | null;
  exePath?: string | null;
  configPath?: string | null;
  poolPath?: string | null;
  entries: ProxyGateEntry[];
  assignments: ProxyGateAssignment[];
  status?: ProxyGateStatus | null;
}

export interface ProxyGateProbeEntry {
  entryId: string;
  displayName: string;
  type: string;
  proxyUrl: string;
  fingerprint: string;
  chainDepth?: number | null;
  health: string;
  latencyMs: number;
  message?: string | null;
}

export interface ProxyGateProbeSnapshot {
  available: boolean;
  error?: string | null;
  revision: string;
  entries: ProxyGateProbeEntry[];
}

export interface AccountNetworkState extends AccountKey {
  status: "pending" | "checking" | "ready" | "interrupted";
  message: string | null;
}

export interface ConflictItem {
  uuid: string;
  providers: ProviderId[];
}

export interface ManagerSnapshot {
  platform: string;
  providers: ProviderInfo[];
  accounts: AccountRecord[];
  selectedProviderId: ProviderId;
  activeAccount: AccountKey | null;
  switchingAccount: AccountKey | null;
  warmAccounts: AccountKey[];
  lastCommitted: AccountKey | null;
  pendingCleanup: AccountKey[];
  conflicts: ConflictItem[];
  sharedProxyAccounts: AccountKey[];
  accountNetworkStates: AccountNetworkState[];
  notice: string | null;
  modalActive: boolean;
}

export interface UpdateCheckResult {
  currentVersion: string;
  latestVersion: string;
  hasUpdate: boolean;
  releaseNotes: string;
  pubDate: string | null;
  downloadUrl: string | null;
  sha256: string | null;
  filename: string | null;
  size: number | null;
  releasePageUrl: string;
}

export interface UpdateProgressEvent {
  stage: string;
  downloaded: number;
  total: number | null;
  percent: number | null;
  message: string;
}

export interface InstallUpdateOutcome {
  status: string;
  message: string;
}
