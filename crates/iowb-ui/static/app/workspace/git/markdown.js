function formatBytes(value) {
  const bytes = Number(value) || 0;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function renderMarkdownLite(value) {
  return renderMarkdownLiteWithSections(value).body;
}

const MOBILE_MARKDOWN_PREVIEW_MAX_CHARS = 48 * 1024;

// Keep web chat on the same Markdown flavor as the Android client: CommonMark
// with GFM tables and strikethrough. Raw HTML stays disabled and links/images
// are restricted below to the same protocols accepted by the mobile renderer.
const mobileMarkdownRenderer = typeof globalThis.markdownit === "function"
  ? globalThis.markdownit({
    html: false,
    breaks: false,
    linkify: false,
    typographer: false,
  })
  : null;

if (mobileMarkdownRenderer) {
  mobileMarkdownRenderer.renderer.rules.link_open = (tokens, index, options, env, self) => {
    const token = tokens[index];
    const href = safeMarkdownUrl(token.attrGet("href") || "");
    if (!href) return "";
    token.attrSet("href", href);
    token.attrSet("target", "_blank");
    token.attrSet("rel", "noreferrer noopener");
    return self.renderToken(tokens, index, options);
  };

  mobileMarkdownRenderer.renderer.rules.image = (tokens, index) => {
    const token = tokens[index];
    const source = safeMarkdownImageUrl(token.attrGet("src") || "");
    if (!source) return escapeHtml(token.content || "attached image");
    const alt = token.content || "attached image";
    return `<img class="markdown-image" src="${escapeHtml(source)}" alt="${escapeHtml(alt)}" />`;
  };

  const renderMobileCodeBlock = (tokens, index) => {
    const content = tokens[index]?.content || "";
    return `<pre class="markdown-code"><code>${escapeHtml(content)}</code></pre>\n`;
  };
  mobileMarkdownRenderer.renderer.rules.fence = renderMobileCodeBlock;
  mobileMarkdownRenderer.renderer.rules.code_block = renderMobileCodeBlock;
}

function normalizeChatHeadingForRender(value) {
  if (typeof normalizeChatToolHeading === "function") {
    return normalizeChatToolHeading(value);
  }
  return String(value || "")
    .trim()
    .replace(/^[#>*\-` ]+/, "")
    .replace(/[#*` ]+$/, "")
    .trim();
}

function isChatBoundaryForRender(value) {
  if (typeof isChatAssistantBoundary === "function") {
    return isChatAssistantBoundary(value);
  }
  return /^(codex|claude|assistant|response)$/i.test(normalizeChatHeadingForRender(value));
}

function isChatTelemetryHeadingForRender(value) {
  if (typeof isChatToolTelemetryHeading === "function") {
    return isChatToolTelemetryHeading(value);
  }
  return /^(?:exec(?:\s*\/\s*(?:parameters|details))?|bash(?:\s*\/\s*(?:parameters|details))?|shell(?:\s+command)?(?:\s*\/\s*(?:parameters|details))?|command_execution|function_call(?:_output)?|custom_tool_call(?:_output)?|tool(?:\s*\/\s*(?:parameters|details))?|(?:edit|create|delete|move)\s*\/\s*.+|file_change(?:\s*\/\s*.+)?|apply[_\s]+patch(?:\s*\/\s*(?:parameters|details))?|patch\s*:.*|diff\s+--git\b.*)$/i
    .test(normalizeChatHeadingForRender(value));
}

function chatDisplaySettingsForRender() {
  return typeof chatDisplaySettings === "function"
    ? chatDisplaySettings()
    : { expandThinking: false, expandParameters: false };
}

function markdownFenceMarkerForMobile(line) {
  const source = String(line || "");
  const indentMatch = source.match(/^[ \t]*/);
  const indent = indentMatch ? indentMatch[0] : "";
  const trimmedStart = source.slice(indent.length);
  if (!trimmedStart.startsWith("```")) return null;
  const tickMatch = trimmedStart.match(/^`+/);
  if (!tickMatch || tickMatch[0].length < 3) return null;
  const suffix = trimmedStart.slice(tickMatch[0].length);
  return {
    indent,
    tickCount: tickMatch[0].length,
    suffix,
    info: suffix.trim(),
  };
}

function nextMarkdownFenceTicksForMobile(activeTicks, line) {
  const marker = markdownFenceMarkerForMobile(line);
  if (!marker) return activeTicks;
  if (activeTicks == null) return marker.tickCount;
  if (!marker.info && marker.tickCount >= activeTicks) return null;
  return activeTicks;
}

function normalizeNestedMarkdownFencesForWeb(lines) {
  const output = [];
  let markdownFence = null;
  let nestedFenceTicks = null;
  for (const line of lines) {
    const fence = markdownFenceMarkerForMobile(line);
    const activeMarkdownFence = markdownFence;
    if (activeMarkdownFence == null) {
      if (fence && fence.tickCount === 3 && ["markdown", "md", "mdown"].includes(
        fence.info.split(" ", 1)[0].trim().toLowerCase(),
      )) {
        markdownFence = fence;
        output.push(`${fence.indent}\`\`\`\`${fence.suffix}`);
      } else {
        output.push(line);
      }
      continue;
    }

    if (fence) {
      if (nestedFenceTicks == null && fence.info) {
        nestedFenceTicks = fence.tickCount;
        output.push(line);
        continue;
      }
      if (
        nestedFenceTicks != null &&
        !fence.info &&
        fence.tickCount >= nestedFenceTicks
      ) {
        nestedFenceTicks = null;
        output.push(line);
        continue;
      }
      if (
        nestedFenceTicks == null &&
        !fence.info &&
        fence.tickCount >= activeMarkdownFence.tickCount
      ) {
        markdownFence = null;
        output.push(`${fence.indent}\`\`\`\``);
        continue;
      }
    }
    output.push(line);
  }
  return output;
}

function markdownPreviewForWeb(value) {
  const source = String(value || "");
  if (source.length <= MOBILE_MARKDOWN_PREVIEW_MAX_CHARS) return source;
  const prefix = source.slice(0, MOBILE_MARKDOWN_PREVIEW_MAX_CHARS).trimEnd();
  const fenceTicks = prefix
    .split("\n")
    .reduce(nextMarkdownFenceTicksForMobile, null);
  return `${prefix}${fenceTicks == null ? "" : `\n${"`".repeat(fenceTicks)}`}\n\n` +
    `[Markdown preview truncated: ${source.length - prefix.length} chars not shown. ` +
    "Copy the response for the full text.]";
}

function assistantToolTimelineHeadingForWeb(value) {
  const normalized = normalizeChatHeadingForRender(value);
  const lower = normalized.toLowerCase();
  const heading = (title) => `### ${title}`;
  if (["codex", "claude", "assistant", "response"].includes(lower)) {
    return heading("Response");
  }
  if (lower === "tokens used") return heading("Token usage");
  for (const [action, title] of [
    ["edit", "Code edited"],
    ["create", "Code created"],
    ["delete", "Code deleted"],
    ["move", "Code moved"],
  ]) {
    if (lower.startsWith(`${action} /`)) {
      return `${heading(title)} · \`${normalized.substring(normalized.indexOf("/") + 1).trim()}\``;
    }
  }
  for (const [prefix, title] of [
    ["add file:", "Code created"],
    ["update file:", "Code edited"],
    ["delete file:", "Code deleted"],
    ["move to:", "Code moved"],
  ]) {
    if (lower.startsWith(prefix)) {
      return `${heading(title)} · \`${normalized.substring(prefix.length).trim()}\``;
    }
  }
  if (
    lower.startsWith("apply patch") ||
    lower.startsWith("apply_patch") ||
    lower.startsWith("patch:")
  ) {
    return heading("Code edit");
  }
  for (const prefix of ["diff /", "tool /", "exec", "bash", "shell", "function_call", "custom_tool_call"]) {
    if (!lower.startsWith(prefix)) continue;
    if (prefix === "diff /" || prefix === "tool /") {
      const detail = normalized.substring(normalized.indexOf("/") + 1).trim();
      return detail ? `${heading("Tool use")} · ${detail[0].toUpperCase()}${detail.slice(1)}` : heading("Tool use");
    }
    const slash = normalized.indexOf("/");
    const detail = slash < 0 ? "" : normalized.slice(slash + 1).trim();
    return detail
      ? `${heading("Tool use · Command")} · ${detail}`
      : heading("Tool use · Command");
  }
  if (lower === "command_execution") return heading("Tool use · Command");
  if (isChatTelemetryHeadingForRender(normalized)) return `${heading("Tool use")} · ${normalized}`;
  return null;
}

function formatAssistantMarkdownForWeb(value) {
  const normalized = markdownPreviewForWeb(
    String(value || "")
      .replace(/\u001B(?:[@-_]|\[[0-?]*[ -/]*[@-~])/g, "")
      .replace(/\r\n?/g, "\n"),
  );
  const lines = normalizeNestedMarkdownFencesForWeb(normalized.split("\n"));
  const output = [];
  let index = 0;
  let fenceTicks = null;
  const commandLine = /^(?:command|run|running command|shell command|executing)\s*[:>]\s*(.+)$/i;
  const fileLine = /^(file|created|updated|modified|deleted|renamed)\s*:\s*(\S.+)$/i;
  const thinkingLine = /^(thinking|reasoning|analysis)\s*:?[ \t]*(.*)$/i;
  const runtimeField = /^(workdir|model|provider|approval|sandbox|reasoning effort|session id)\s*:\s*(.+)$/i;

  while (index < lines.length) {
    const line = lines[index];
    const trimmed = line.trim();
    const fence = markdownFenceMarkerForMobile(line);
    if (fence) {
      const activeFenceTicks = fenceTicks;
      fenceTicks = activeFenceTicks == null
        ? fence.tickCount
        : (!fence.info && fence.tickCount >= activeFenceTicks ? null : activeFenceTicks);
      output.push(line);
      index += 1;
      continue;
    }
    if (
      fenceTicks == null &&
      (trimmed.startsWith("diff --git ") || trimmed.startsWith("@@ "))
    ) {
      const diff = [];
      while (index < lines.length) {
        const candidate = lines[index];
        if (diff.length && !candidate.trim()) break;
        diff.push(candidate);
        index += 1;
      }
      output.push("```diff", ...diff, "```");
      continue;
    }
    if (fenceTicks == null) {
      const timelineHeading = assistantToolTimelineHeadingForWeb(trimmed);
      if (timelineHeading) {
        output.push(timelineHeading);
        index += 1;
        continue;
      }
      const command = trimmed.match(commandLine)?.[1];
      if (command) {
        output.push("### Command", "```sh", command, "```");
        index += 1;
        continue;
      }
      const file = trimmed.match(fileLine);
      if (file) {
        const action = file[1].toLowerCase();
        const path = file[2];
        output.push(action === "file" ? `**File:** \`${path}\`` : `### Code edited · \`${path}\``);
        index += 1;
        continue;
      }
      const thinking = trimmed.match(thinkingLine);
      if (thinking) {
        output.push(`### ${thinking[1][0].toUpperCase()}${thinking[1].slice(1)}`);
        if (thinking[2].trim()) output.push("", thinking[2]);
        index += 1;
        continue;
      }
      const runtime = trimmed.match(runtimeField);
      if (runtime) {
        output.push(`- **${runtime[1]}:** \`${runtime[2]}\``);
        index += 1;
        continue;
      }
      if (/^ERROR:/i.test(trimmed)) {
        output.push(`> **Error:** ${trimmed.slice(trimmed.indexOf(":") + 1).trim()}`);
        index += 1;
        continue;
      }
    }
    output.push(line);
    index += 1;
  }
  return output.join("\n").trim();
}

// Parse a chat bubble into a series of markdown segments interleaved with
// structured `exec / Parameters` / `exec / Details` (Codex) or
// `tool / Parameters` / `tool / Details` (generic tool normalizer)
// collapsibles, plus a collapsible `thinking` block for the model's
// chain-of-thought. The chat UI used to render them all as plain text,
// which made long tool calls and reasoning blocks unreadable. The non-exec
// parts keep their original Markdown rendering so headings, bold, code
// fences, and lists still work.
function renderMarkdownLiteWithSections(value, options = {}) {
  const source = options.assistant === true
    ? formatAssistantMarkdownForWeb(value)
    : String(value || "").replace(/\r\n?/g, "\n");
  const lines = source.split("\n");
  const sections = [];
  let buffer = [];
  let currentSection = null;
  let fenceTicks = null;

  const flushBuffer = () => {
    if (!buffer.length) return;
    sections.push({ kind: "markdown", text: buffer.join("\n") });
    buffer = [];
  };

  const phaseLabel = (variant) => variant === "parameters" ? "Parameters" : "Details";

  const markdownFenceMarker = (line) => {
    const trimmedStart = String(line || "").trimStart();
    if (!trimmedStart.startsWith("```")) return null;
    const tickMatch = trimmedStart.match(/^`{3,}/);
    if (!tickMatch) return null;
    return {
      tickCount: tickMatch[0].length,
      info: trimmedStart.slice(tickMatch[0].length).trim(),
    };
  };

  const nextFenceTicks = (activeTicks, line) => {
    const marker = markdownFenceMarker(line);
    if (!marker) return activeTicks;
    if (activeTicks == null) return marker.tickCount;
    if (!marker.info && marker.tickCount >= activeTicks) return null;
    return activeTicks;
  };

  const thinkingHeader = (line) => {
    const trimmed = String(line || "").trim();
    const direct = trimmed.match(/^(thinking|reasoning|analysis)\s*:?[ \t]*(.*)$/i);
    const canonical = trimmed.match(/^###\s+(thinking|reasoning|analysis)\s*$/i);
    const match = direct || canonical;
    if (match) return `${match[1][0].toUpperCase()}${match[1].slice(1)}`;
    return "";
  };

  const toolNameFromMarkdown = (text) => {
    for (const line of String(text || "").split("\n")) {
      const match = line.trim().match(/^\*\*Tool:\*\*\s*`([^`]+)`\s*$/i);
      if (match?.[1]?.trim()) return match[1].trim();
    }
    return "";
  };

  const toolNameFromTitle = (title) => {
    const match = String(title || "").trim().match(/^(.+?)\s*·\s*(?:Parameters|Details)$/i);
    const name = match?.[1]?.trim() || "";
    if (!name || /^Tool use(?:\s*·\s*Command)?$/i.test(name)) return "";
    return name;
  };

  const titleWithPhase = (title, variant) => {
    const phase = phaseLabel(variant);
    if (title && /\b(?:Parameters|Details)$/i.test(title)) {
      return title.replace(/\b(?:Parameters|Details)$/i, phase);
    }
    return `Tool use · ${phase}`;
  };

  const codeEditTitle = (action, path) => {
    const cleanPath = String(path || "").trim();
    if (/^create$/i.test(action)) return cleanPath ? `Code created · \`${cleanPath}\`` : "Code created";
    if (/^delete$/i.test(action)) return cleanPath ? `Code deleted · \`${cleanPath}\`` : "Code deleted";
    if (/^move$/i.test(action)) return cleanPath ? `Code moved · \`${cleanPath}\`` : "Code moved";
    return cleanPath ? `Code edited · \`${cleanPath}\`` : "Code edit";
  };

  const toolSectionHeader = (line) => {
    const trimmed = String(line || "").trim();
    const canonicalHeading = trimmed.match(/^###\s+(.+)$/);
    const normalized = canonicalHeading
      ? canonicalHeading[1].trim()
      : normalizeChatHeadingForRender(trimmed);
    let match = normalized.match(/^(exec|bash|shell(?:\s+command)?|command_execution|function_call(?:_output)?|custom_tool_call(?:_output)?)(?:\s*\/\s*(Parameters|Details))?\s*$/i);
    if (match) {
      const variant = (match[2] || "Parameters").toLowerCase();
      return {
        kind: "exec",
        variant,
        title: `Tool use · Command · ${phaseLabel(variant)}`,
        toolish: true,
      };
    }
    match = normalized.match(/^(tool|diff)\s*\/\s*(Parameters|Details)\s*$/i);
    if (match) {
      const variant = match[2].toLowerCase();
      return {
        kind: "tool",
        variant,
        title: `Tool use · ${phaseLabel(variant)}`,
        toolish: true,
      };
    }
    match = normalized.match(/^Tool use(?:\s*·\s*Command)?\s*·\s*(Parameters|Details)\s*$/i);
    if (match) {
      const variant = match[1].toLowerCase();
      const kind = /\bCommand\b/i.test(normalized) ? "exec" : (currentSection?.kind || "tool");
      return {
        kind,
        variant,
        title: /\bCommand\b/i.test(normalized)
          ? `Tool use · Command · ${phaseLabel(variant)}`
          : `Tool use · ${phaseLabel(variant)}`,
        toolish: true,
      };
    }
    match = canonicalHeading && normalized.match(/^Tool use(?:\s*·\s*Command)?(?:\s*·\s*(Parameters|Details))?\s*$/i);
    if (match) {
      const variant = (match[1] || "Parameters").toLowerCase();
      const kind = /\bCommand\b/i.test(normalized) ? "exec" : "tool";
      return {
        kind,
        variant,
        title: /\bCommand\b/i.test(normalized)
          ? `Tool use · Command · ${phaseLabel(variant)}`
          : `Tool use · ${phaseLabel(variant)}`,
        toolish: true,
      };
    }
    match = canonicalHeading && normalized.match(/^(.+?)\s*·\s*(Parameters|Details)\s*$/i);
    if (match) {
      const variant = match[2].toLowerCase();
      return {
        kind: "tool",
        variant,
        title: `${match[1].trim()} · ${phaseLabel(variant)}`,
        toolish: true,
      };
    }
    match = canonicalHeading && normalized.match(/^Code\s+(?:created|deleted|moved|edited|edit)(?:\s*·\s*.+)?$/i);
    if (match) {
      return {
        kind: "code",
        variant: "details",
        title: normalized,
        toolish: false,
      };
    }
    match = normalized.match(/^(?:Command\s*·\s*)?(Parameters|Details)\s*$/i);
    if (match && currentSection?.toolish) {
      const variant = match[1].toLowerCase();
      return {
        kind: currentSection.kind,
        variant,
        title: titleWithPhase(currentSection.title, variant),
        toolish: true,
      };
    }
    match = normalized.match(/^(edit|create|delete|move)\s*\/\s*(.+)$/i);
    if (match) {
      return {
        kind: "code",
        variant: "details",
        title: codeEditTitle(match[1], match[2]),
        toolish: false,
      };
    }
    match = normalized.match(/^(add file|update file|delete file|move to)\s*:\s*(.+)$/i);
    if (match) {
      const action = match[1].toLowerCase().startsWith("add")
        ? "create"
        : match[1].toLowerCase().startsWith("delete")
          ? "delete"
          : match[1].toLowerCase().startsWith("move")
            ? "move"
            : "edit";
      return {
        kind: "code",
        variant: "details",
        title: codeEditTitle(action, match[2]),
        toolish: false,
      };
    }
    match = normalized.match(/^(apply[_\s]+patch|patch\s*:.*|file_change(?:\s*\/\s*.+)?)(?:\s*\/\s*(Parameters|Details))?$/i);
    if (match) {
      return {
        kind: "code",
        variant: (match[2] || "Details").toLowerCase(),
        title: "Code edit",
        toolish: false,
      };
    }
    return null;
  };

  const formatSectionBody = (section) => {
    const sourceLines = section.lines.slice();
    if (!section.toolish || section.variant !== "parameters") {
      return sourceLines.join("\n").trim();
    }
    let activeFenceTicks = null;
    return sourceLines.map((line) => {
      const commandHeading = activeFenceTicks == null &&
        normalizeChatHeadingForRender(line).match(/^Command$/i) &&
        line.trimStart().startsWith("#");
      const output = commandHeading ? "**Command:**" : line;
      activeFenceTicks = nextFenceTicks(activeFenceTicks, line);
      return output;
    }).join("\n").trim();
  };

  const canonicalizeToolTitles = () => {
    let activeToolName = "";
    for (const section of sections) {
      if (!section || section.kind === "markdown" || section.kind === "thinking") continue;
      if (!section.toolish) {
        section.displayTitle = section.title || "Activity";
        continue;
      }
      const body = section.lines.join("\n");
      const toolName = toolNameFromMarkdown(body) || toolNameFromTitle(section.title) || activeToolName;
      if (toolName) activeToolName = toolName;
      const phase = phaseLabel(section.variant);
      if (toolName) {
        section.displayTitle = `${toolName} · ${phase}`;
      } else if (/^Tool use\s*·\s*Command\s*·/i.test(section.title || "")) {
        section.displayTitle = `Tool use · ${phase}`;
      } else {
        section.displayTitle = section.title || `Tool use · ${phase}`;
      }
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (fenceTicks == null && /^###\s+Response\s*$/i.test(line.trim())) {
      flushBuffer();
      currentSection = null;
      continue;
    }
    const header = fenceTicks == null ? toolSectionHeader(line) : null;
    if (header) {
      flushBuffer();
      currentSection = { ...header, lines: [] };
      sections.push(currentSection);
      continue;
    }
    const thinkingTitle = fenceTicks == null ? thinkingHeader(line) : "";
    if (thinkingTitle) {
      // `thinking` starts a reasoning block. Everything after the header
      // until the next blank line that precedes a non-thinking segment
      // (or the next structured header) belongs to the block.
      flushBuffer();
      currentSection = { kind: "thinking", variant: "block", title: thinkingTitle, lines: [] };
      sections.push(currentSection);
      continue;
    }
    if (currentSection && fenceTicks == null && isChatBoundaryForRender(line)) {
      currentSection = null;
      continue;
    }
    if (currentSection) {
      currentSection.lines.push(line);
    } else {
      buffer.push(line);
    }
    fenceTicks = nextFenceTicks(fenceTicks, line);
  }
  flushBuffer();
  canonicalizeToolTitles();

  const html = [];
  for (const section of sections) {
    if (section.kind === "markdown") {
      html.push(renderMarkdownSegment(section.text));
    } else if (section.kind === "thinking") {
      const body = section.lines.join("\n").trim();
      html.push(
        `<details class="thinking-section"${chatDisplaySettingsForRender().expandThinking ? " open" : ""}>` +
          `<summary><span class="thinking-title">${escapeHtml(section.title || "Thinking")}</span></summary>` +
          `<div class="thinking-body">${renderMarkdownSegment(
            body || "*No reasoning captured.*"
          )}</div>` +
          `</details>`
      );
    } else {
      const variant = section.variant === "parameters" ? "parameters" : "details";
      const label = section.displayTitle || `${section.kind} / ${phaseLabel(variant)}`;
      const body = formatSectionBody(section);
      const open = variant === "parameters" && chatDisplaySettingsForRender().expandParameters;
      html.push(
        `<details class="exec-section exec-${variant}"${open ? " open" : ""}>` +
          `<summary><span class="exec-title">${escapeHtml(label)}</span></summary>` +
          `<div class="exec-body">${renderMarkdownSegment(
            body || "*No data captured.*"
          )}</div>` +
          `</details>`
      );
    }
  }
  return { body: html.join(""), sections };
}

function renderMarkdownSegment(value) {
  if (mobileMarkdownRenderer) {
    return mobileMarkdownRenderer.render(String(value || ""));
  }
  const lines = String(value || "").replace(/\r\n?/g, "\n").split("\n");
  const html = [];
  let inCode = false;
  let listMode = "";
  const closeList = () => {
    if (!listMode) return;
    html.push(`</${listMode}>`);
    listMode = "";
  };
  const openList = (mode) => {
    if (listMode === mode) return;
    closeList();
    listMode = mode;
    html.push(`<${mode}>`);
  };

  for (const line of lines) {
    if (line.trim().startsWith("```")) {
      closeList();
      html.push(inCode ? "</code></pre>" : `<pre class="markdown-code"><code>`);
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      html.push(`${escapeHtml(line)}\n`);
      continue;
    }
    if (!line.trim()) {
      closeList();
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      closeList();
      const level = Math.min(4, heading[1].length + 2);
      html.push(`<h${level}>${renderMarkdownInline(heading[2])}</h${level}>`);
      continue;
    }
    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      closeList();
      html.push(`<blockquote>${renderMarkdownInline(quote[1])}</blockquote>`);
      continue;
    }
    const unordered = line.match(/^\s*[-*]\s+(.+)$/);
    if (unordered) {
      openList("ul");
      html.push(`<li>${renderMarkdownInline(unordered[1])}</li>`);
      continue;
    }
    const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
    if (ordered) {
      openList("ol");
      html.push(`<li>${renderMarkdownInline(ordered[1])}</li>`);
      continue;
    }
    closeList();
    html.push(`<p>${renderMarkdownInline(line)}</p>`);
  }
  closeList();
  if (inCode) html.push("</code></pre>");
  return html.join("");
}

function renderMarkdownInline(value) {
  const pattern = /(!\[([^\]\n]*)\]\(([^)\s]+)\)|`([^`]+)`|\*\*([^*]+)\*\*|__([^_]+)__|\[([^\]\n]+)\]\(([^)\s]+)\))/g;
  let html = "";
  let index = 0;
  for (const match of value.matchAll(pattern)) {
    html += escapeHtml(value.slice(index, match.index));
    if (match[2] && match[3]) {
      const source = safeMarkdownImageUrl(match[3]);
      html += source
        ? `<img class="markdown-image" src="${escapeHtml(source)}" alt="${escapeHtml(match[2] || "attached image")}" />`
        : escapeHtml(match[0]);
    } else if (match[4] !== undefined) {
      html += `<code>${escapeHtml(match[4])}</code>`;
    } else if (match[5] !== undefined || match[6] !== undefined) {
      html += `<strong>${escapeHtml(match[5] ?? match[6])}</strong>`;
    } else if (match[7] !== undefined && match[8] !== undefined) {
      const href = safeMarkdownUrl(match[8]);
      html += href
        ? `<a href="${escapeHtml(href)}" target="_blank" rel="noreferrer noopener">${escapeHtml(match[7])}</a>`
        : escapeHtml(match[0]);
    }
    index = match.index + match[0].length;
  }
  html += escapeHtml(value.slice(index));
  return html;
}

function safeMarkdownUrl(raw) {
  const trimmed = String(raw || "").trim();
  if (!trimmed) return "";
  const scheme = trimmed.match(/^([a-z][a-z\d+.-]*):/i)?.[1]?.toLowerCase();
  return !scheme || ["http", "https", "mailto"].includes(scheme) ? trimmed : "";
}

function safeMarkdownImageUrl(raw) {
  const trimmed = String(raw || "").trim();
  const lower = trimmed.toLowerCase();
  const scheme = lower.match(/^([a-z][a-z\d+.-]*):/)?.[1] || "";
  return (!scheme && trimmed) ||
    lower.startsWith("http://") ||
    lower.startsWith("https://") ||
    lower.startsWith("data:image/png;") ||
    lower.startsWith("data:image/jpeg;") ||
    lower.startsWith("data:image/jpg;") ||
    lower.startsWith("data:image/gif;") ||
    lower.startsWith("data:image/webp;") ||
    lower.startsWith("data:image/bmp;")
    ? trimmed
    : "";
}
