const IO_GATEWAY_API_PATH_SUFFIXES = [
  "/v1",
  "/codex",
  "/claude",
  "/agw",
  "/gemini",
  "/qwen",
  "/deepseek",
  "/grok",
  "/minimax",
  "/copilot",
  "/glm",
];

function ioGatewayRootUrl(value) {
  let root = String(value || "").trim().replace(/\/+$/, "");
  while (root) {
    const lower = root.toLowerCase();
    const suffix = IO_GATEWAY_API_PATH_SUFFIXES.find((candidate) => lower.endsWith(candidate));
    if (!suffix) return root;
    root = root.slice(0, -suffix.length).replace(/\/+$/, "");
  }
  return root;
}

function normalizeIoGatewayUrl(value) {
  const trimmed = String(value || "").trim().replace(/\/+$/, "");
  if (!trimmed.startsWith("http://") && !trimmed.startsWith("https://")) {
    throw new Error("Gateway URL must start with http:// or https://.");
  }
  return ioGatewayRootUrl(trimmed);
}

function normalizeTotpSecret(value) {
  const trimmed = String(value || "").trim();
  let secret = trimmed;
  if (trimmed.toLowerCase().startsWith("otpauth://")) {
    try {
      secret = new URL(trimmed).searchParams.get("secret") || "";
    } catch {
      secret = "";
    }
  }
  return secret.replace(/[\s-]/g, "").toUpperCase();
}

function decodeBase32(value) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const normalized = String(value || "").replace(/=+$/, "");
  const output = [];
  let buffer = 0;
  let bits = 0;
  for (const character of normalized) {
    const digit = alphabet.indexOf(character.toUpperCase());
    if (digit < 0) throw new Error("Secret OTP must be valid Base32.");
    buffer = (buffer << 5) | digit;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      output.push((buffer >> bits) & 0xff);
      buffer &= bits ? (1 << bits) - 1 : 0;
    }
  }
  return output;
}

function validateTotpSecret(secret) {
  if (decodeBase32(secret).length < 10) {
    throw new Error("Secret OTP must be a valid Base32 TOTP secret.");
  }
}

function ioGatewayConfigured(status, key) {
  const config = status?.config || {};
  if (key === "gatewayApiKey") {
    return status?.apiKeyConfigured === true || config.gatewayApiKeyConfigured === true;
  }
  return status?.gatewayOtpConfigured === true || config.gatewayOtpConfigured === true;
}

function ioGatewaySummaryRow(label, value, tone = "") {
  const badge = tone ? ` class="badge ${tone}"` : "";
  return `<div><dt>${escapeHtml(label)}</dt><dd${badge}>${escapeHtml(value)}</dd></div>`;
}

function renderIoGatewayConfig(status) {
  state.ioGatewayStatus = status;
  state.lastSettingsRows = null;
  const config = status?.config || {};
  const effective = status?.effective || {};
  const gatewayUrl = firstDefined(config.gatewayUrl, ioGatewayRootUrl(effective.baseUrl), "unset");
  const apiKeyConfigured = ioGatewayConfigured(status, "gatewayApiKey");
  const otpConfigured = ioGatewayConfigured(status, "gatewayOtpSecret");
  const runtimeReady = status?.runtimeReady === true;
  const summary = qs("#io-gateway-summary");
  if (summary) {
    summary.innerHTML = [
      ioGatewaySummaryRow("Chat runtime", effective.chatRuntime === "io_gateway" ? "IO Gateway" : "Native CLI"),
      ioGatewaySummaryRow("Mode", effective.mode || "off"),
      ioGatewaySummaryRow("Gateway URL", gatewayUrl),
      ioGatewaySummaryRow("API Key", apiKeyConfigured ? "configured" : "missing", apiKeyConfigured ? "ok" : "warn"),
      ioGatewaySummaryRow("Secret OTP", otpConfigured ? "configured" : "missing", otpConfigured ? "ok" : "warn"),
      ioGatewaySummaryRow("Model", firstDefined(config.model, effective.model, "unset")),
      ioGatewaySummaryRow("Runtime", runtimeReady ? "ready" : "not ready", runtimeReady ? "ok" : "warn"),
    ].join("");
  }
  const enabled = qs("#io-gateway-enabled");
  const url = qs("#io-gateway-url");
  if (enabled) enabled.checked = effective.chatRuntime === "io_gateway";
  if (url) url.value = gatewayUrl === "unset" ? "" : gatewayUrl;
  const apiKey = qs("#io-gateway-api-key");
  const otp = qs("#io-gateway-otp-secret");
  if (apiKey && !apiKey.value) {
    apiKey.placeholder = apiKeyConfigured ? "Configured — click Show to load" : "Gateway API key";
  }
  if (otp && !otp.value) {
    otp.placeholder = otpConfigured ? "Configured — click Show to load" : "Base32 TOTP secret";
  }
  renderJson("#settings-json", status);
}

async function loadIoGatewayConfig(options = {}) {
  if (state.ioGatewayLoadPromise && !options.force) return state.ioGatewayLoadPromise;
  const generation = ++state.ioGatewayLoadGeneration;
  const request = api("/api/settings/direct-ai")
    .then((status) => {
      if (generation === state.ioGatewayLoadGeneration) renderIoGatewayConfig(status);
      return status;
    })
    .finally(() => {
      if (state.ioGatewayLoadPromise === request) state.ioGatewayLoadPromise = null;
    });
  state.ioGatewayLoadPromise = request;
  return request;
}

function ioGatewaySecretControl(secretKey) {
  if (secretKey === "gatewayApiKey") {
    return {
      input: qs("#io-gateway-api-key"),
      button: qs("#io-gateway-api-key-toggle"),
      title: "API Key",
    };
  }
  return {
    input: qs("#io-gateway-otp-secret"),
    button: qs("#io-gateway-otp-secret-toggle"),
    title: "Secret OTP",
  };
}

function setIoGatewaySecretVisibility(secretKey, visible) {
  const { input, button, title } = ioGatewaySecretControl(secretKey);
  if (!input || !button) return;
  input.type = visible ? "text" : "password";
  button.dataset.symbol = visible ? "eye-off" : "eye";
  button.setAttribute("aria-label", `${visible ? "Hide" : "Show"} ${title}`);
  button.title = `${visible ? "Hide" : "Show"} ${title}`;
}

async function toggleIoGatewaySecret(secretKey) {
  const { input, button, title } = ioGatewaySecretControl(secretKey);
  if (!input || !button) return;
  if (input.value) {
    setIoGatewaySecretVisibility(secretKey, input.type === "password");
    input.focus();
    return;
  }
  await withButtonLoading(button, async () => {
    const body = await api("/api/settings/direct-ai?revealSecrets=true");
    const secret = String(body?.secrets?.[secretKey] || "");
    if (!secret) {
      showToast(`${title} is not configured`, "warn");
      return;
    }
    input.value = secret;
    setIoGatewaySecretVisibility(secretKey, true);
    input.focus();
  });
}

async function saveIoGatewayConfig(event) {
  event.preventDefault();
  const normalizedUrl = normalizeIoGatewayUrl(qs("#io-gateway-url")?.value);
  const key = qs("#io-gateway-api-key")?.value.trim() || "";
  const secret = normalizeTotpSecret(qs("#io-gateway-otp-secret")?.value);
  const useIoGateway = qs("#io-gateway-enabled")?.checked === true;
  const serverApiKeyConfigured = ioGatewayConfigured(state.ioGatewayStatus, "gatewayApiKey");
  if (useIoGateway && !key && !serverApiKeyConfigured) {
    throw new Error("API key is required when IO Gateway is selected.");
  }
  if (secret) validateTotpSecret(secret);
  const storedConfig = state.ioGatewayStatus?.config || {};
  const payload = {
    mode: "aiproxy",
    chatRuntime: useIoGateway ? "io_gateway" : "native_cli",
    baseUrl: `${normalizedUrl}/claude`,
    model: storedConfig.model ?? null,
    gatewayUrl: normalizedUrl,
  };
  if (key) payload.gatewayApiKey = key;
  if (secret) payload.gatewayOtpSecret = secret;
  await api("/api/settings/direct-ai", {
    method: "PUT",
    body: JSON.stringify(payload),
  });
  if (qs("#io-gateway-url")) qs("#io-gateway-url").value = normalizedUrl;
  if (qs("#io-gateway-api-key")) qs("#io-gateway-api-key").value = key;
  if (qs("#io-gateway-otp-secret")) qs("#io-gateway-otp-secret").value = secret;
  setIoGatewaySecretVisibility("gatewayApiKey", false);
  setIoGatewaySecretVisibility("gatewayOtpSecret", false);
  state.ioGatewayStatus = null;
  await loadIoGatewayConfig({ force: true });
  showToast("IO Gateway settings saved on server", "ok");
}

function defaultNotificationPreferences() {
  return {
    channels: {
      browser: true,
      webPush: false,
    },
    events: {
      sessionComplete: true,
      permissionRequired: true,
      processFailed: true,
    },
  };
}

function normalizeNotificationPreferences(preferences = {}) {
  const defaults = defaultNotificationPreferences();
  return {
    channels: {
      ...defaults.channels,
      ...(preferences.channels || {}),
    },
    events: {
      ...defaults.events,
      ...(preferences.events || {}),
    },
  };
}

function renderNotificationPreferences(preferences) {
  const normalized = normalizeNotificationPreferences(preferences);
  state.notificationPreferences = normalized;
  state.lastSettingsRows = null;
  qs("#notify-browser").checked = !!normalized.channels.browser;
  qs("#notify-web-push").checked = !!normalized.channels.webPush;
  qs("#notify-session-complete").checked = !!normalized.events.sessionComplete;
  qs("#notify-permission-required").checked = !!normalized.events.permissionRequired;
  qs("#notify-process-failed").checked = !!normalized.events.processFailed;
  qs("#settings-json-input").value = JSON.stringify(normalized, null, 2);
  updateNotificationStatus();
  const enabledEvents = Object.entries(normalized.events)
    .filter(([, enabled]) => enabled)
    .map(([event]) => event);
  const enabledChannels = Object.entries(normalized.channels)
    .filter(([, enabled]) => enabled)
    .map(([channel]) => channel);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>Notification Preferences</strong>
      <span class="badge ${enabledChannels.length ? "ok" : "warn"}">${enabledChannels.length ? "enabled" : "muted"}</span>
    </header>
    <span class="meta">Channels: ${escapeHtml(enabledChannels.join(", ") || "none")}</span>
    <span class="meta">Events: ${escapeHtml(enabledEvents.join(", ") || "none")}</span>
  </article>`;
}

function notificationPreferencesFromControls() {
  return {
    channels: {
      browser: qs("#notify-browser").checked,
      webPush: qs("#notify-web-push").checked,
    },
    events: {
      sessionComplete: qs("#notify-session-complete").checked,
      permissionRequired: qs("#notify-permission-required").checked,
      processFailed: qs("#notify-process-failed").checked,
    },
  };
}

function updateNotificationStatus() {
  const status = qs("#notification-status");
  if (!status) return;
  status.className = "badge warn";
  if (!("Notification" in window)) {
    status.textContent = "Unsupported";
    return;
  }
  const permission = Notification.permission;
  status.textContent = permission;
  status.className = `badge ${permission === "granted" ? "ok" : permission === "denied" ? "danger" : "warn"}`;
}

async function saveNotificationPreferences() {
  const preferences = notificationPreferencesFromControls();
  const body = await api("/api/settings/notification-preferences", {
    method: "PUT",
    body: JSON.stringify(preferences),
  });
  renderSettingsResponse(body);
}

async function requestNotificationPermission() {
  if (!("Notification" in window)) {
    updateNotificationStatus();
    return;
  }
  await Notification.requestPermission();
  updateNotificationStatus();
}

async function previewBrowserNotification() {
  if (!("Notification" in window)) {
    updateNotificationStatus();
    return;
  }
  if (Notification.permission !== "granted") {
    await requestNotificationPermission();
  }
  if (Notification.permission !== "granted") return;
  const title = "io-workbench";
  const options = {
    body: "Browser notifications are enabled.",
    tag: "iowb-preview",
  };
  let registration = null;
  if (navigator.serviceWorker?.ready) {
    registration = await navigator.serviceWorker.ready.catch(() => null);
  }
  if (registration?.showNotification) {
    await registration.showNotification(title, options);
  } else {
    new Notification(title, options);
  }
}

async function testPushNotificationCommand() {
  const body = await api("/api/notifications/test", {
    method: "POST",
    body: JSON.stringify({
      title: "io-workbench",
      body: "Test notification",
      preferences: notificationPreferencesFromControls(),
    }),
  });
  renderSettingsResponse(body);
}

function renderApiKeys(apiKeys) {
  state.lastSettingsRows = { type: "apiKeys", rows: apiKeys };
  const rows = filteredItems(apiKeys, qs("#settings-filter")?.value || "", [
    "keyName",
    "keyPrefix",
    "api_key",
    (key) => key.isActive ? "active" : "inactive",
  ]);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  if (!rows.length) {
    target.innerHTML = '<p class="empty">No API keys.</p>';
    return;
  }
  const visible = rows.slice(0, state.limits.settingsRows);
  target.innerHTML = visible.map((key) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(key.keyName)}</strong>
      <span class="badge ${key.isActive ? "ok" : "warn"}">${key.isActive ? "active" : "inactive"}</span>
    </header>
    <span class="meta">ID ${key.id} · ${escapeHtml(key.keyPrefix || key.api_key || "")} · ${escapeHtml(formatDate(key.createdAt))}</span>
    <div class="row-actions">
      <button type="button" data-settings-action="api-key-toggle" data-settings-id="${key.id}" data-settings-value="${key.isActive ? "false" : "true"}">Toggle</button>
      <button type="button" data-settings-action="api-key-delete" data-settings-id="${key.id}">Delete</button>
    </div>
  </article>`).join("") + showMoreButton("settingsRows", rows.length, "settingsRows");
  bindSettingsActionButtons(target);
  bindShowMore(target);
}

function renderCreatedApiKey(apiKey) {
  state.lastSettingsRows = null;
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(apiKey.keyName || "API Key")}</strong>
      <span class="badge ok">created</span>
    </header>
    <span class="meta">ID ${apiKey.id} · ${escapeHtml(apiKey.keyPrefix || "")}</span>
    <pre>${escapeHtml(apiKey.api_key || apiKey.apiKey || "")}</pre>
    <button type="button" data-copy-text="${escapeHtml(apiKey.api_key || apiKey.apiKey || "")}">Copy Key</button>
  </article>`;
  bindCopyButtons(target);
}

function renderCredentials(credentials) {
  state.lastSettingsRows = { type: "credentials", rows: credentials };
  const rows = filteredItems(credentials, qs("#settings-filter")?.value || "", [
    "credentialName",
    "credentialType",
    "description",
    (credential) => credential.isActive ? "active" : "inactive",
  ]);
  const target = qs("#settings-json");
  target.className = "output-panel result-list";
  if (!rows.length) {
    target.innerHTML = '<p class="empty">No credentials.</p>';
    return;
  }
  const visible = rows.slice(0, state.limits.settingsRows);
  target.innerHTML = visible.map((credential) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(credential.credentialName)}</strong>
      <span class="badge ${credential.isActive ? "ok" : "warn"}">${credential.isActive ? "active" : "inactive"}</span>
    </header>
    <span class="meta">ID ${credential.id} · ${escapeHtml(credential.credentialType)} · ${escapeHtml(formatDate(credential.updatedAt))}</span>
    ${credential.description ? `<span>${escapeHtml(credential.description)}</span>` : ""}
    <div class="row-actions">
      <button type="button" data-settings-action="credential-toggle" data-settings-id="${credential.id}" data-settings-value="${credential.isActive ? "false" : "true"}">Toggle</button>
      <button type="button" data-settings-action="credential-delete" data-settings-id="${credential.id}">Delete</button>
    </div>
  </article>`).join("") + showMoreButton("settingsRows", rows.length, "settingsRows");
  bindSettingsActionButtons(target);
  bindShowMore(target);
}

function renderSettingsRows() {
  if (!state.lastSettingsRows) return;
  if (state.lastSettingsRows.type === "apiKeys") {
    renderApiKeys(state.lastSettingsRows.rows);
  } else if (state.lastSettingsRows.type === "credentials") {
    renderCredentials(state.lastSettingsRows.rows);
  }
}

function bindSettingsActionButtons(root) {
  root.querySelectorAll("[data-settings-action]").forEach((button) => {
    button.addEventListener("click", () => {
      qs("#settings-action").value = button.dataset.settingsAction;
      qs("#settings-name").value = button.dataset.settingsId || "";
      qs("#settings-value").value = button.dataset.settingsValue || "";
    });
  });
}

async function loadSettingsView(path) {
  const body = await api(path);
  renderSettingsResponse(body);
}

async function applySettingsAction(event) {
  event.preventDefault();
  const action = qs("#settings-action").value;
  const name = qs("#settings-name").value.trim();
  const type = qs("#settings-type").value.trim();
  const value = qs("#settings-value").value.trim();
  const json = parseJsonField("#settings-json-input", {});
  let body;
  if (action === "api-key") {
    if (!name) return;
    body = await api("/api/settings/api-keys", {
      method: "POST",
      body: JSON.stringify({ keyName: name }),
    });
  } else if (action === "api-key-delete") {
    if (!name) return;
    body = await api(`/api/settings/api-keys/${encodeURIComponent(name)}`, {
      method: "DELETE",
    });
  } else if (action === "api-key-toggle") {
    if (!name) return;
    body = await api(`/api/settings/api-keys/${encodeURIComponent(name)}/toggle`, {
      method: "PATCH",
      body: JSON.stringify({ isActive: value !== "false" }),
    });
  } else if (action === "credential") {
    if (!name || !type || !value) return;
    body = await api("/api/settings/credentials", {
      method: "POST",
      body: JSON.stringify({
        credentialName: name,
        credentialType: type,
        credentialValue: value,
      }),
    });
  } else if (action === "credential-delete") {
    if (!name) return;
    body = await api(`/api/settings/credentials/${encodeURIComponent(name)}`, {
      method: "DELETE",
    });
  } else if (action === "credential-toggle") {
    if (!name) return;
    body = await api(`/api/settings/credentials/${encodeURIComponent(name)}/toggle`, {
      method: "PATCH",
      body: JSON.stringify({ isActive: value !== "false" }),
    });
  } else if (action === "git-config") {
    if (!name || !type) return;
    body = await api("/api/user/git-config", {
      method: "POST",
      body: JSON.stringify({ gitName: name, gitEmail: type }),
    });
  } else if (action === "notification") {
    body = await api("/api/settings/notification-preferences", {
      method: "PUT",
      body: JSON.stringify(json),
    });
  } else if (action === "direct-ai") {
    body = await api("/api/settings/direct-ai", {
      method: "PUT",
      body: JSON.stringify(json),
    });
  } else if (action === "onboarding") {
    body = await api("/api/user/complete-onboarding", {
      method: "POST",
    });
  }
  renderSettingsResponse(body);
}

function parseJsonField(selector, fallback) {
  const value = qs(selector).value.trim();
  if (!value) return fallback;
  return JSON.parse(value);
}
