function formatBytes(value) {
  const bytes = Number(value) || 0;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

function renderMarkdownLite(value) {
  return renderMarkdownLiteWithSections(value).body;
}

// Parse a chat bubble into a series of markdown segments interleaved with
// structured `exec / Parameters` / `exec / Details` (Codex) or
// `tool / Parameters` / `tool / Details` (generic tool normalizer)
// collapsibles, plus a collapsible `thinking` block for the model's
// chain-of-thought. The chat UI used to render them all as plain text,
// which made long tool calls and reasoning blocks unreadable. The non-exec
// parts keep their original Markdown rendering so headings, bold, code
// fences, and lists still work.
function renderMarkdownLiteWithSections(value) {
  const lines = String(value || "").replace(/\r\n?/g, "\n").split("\n");
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
    const normalized = normalizeChatToolHeading(line);
    if (/^thinking$/i.test(normalized)) return "Thinking";
    if (/^reasoning$/i.test(normalized)) return "Reasoning";
    if (/^analysis$/i.test(normalized)) return "Analysis";
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
    const normalized = normalizeChatToolHeading(line);
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
    match = normalized.match(/^(.+?)\s*·\s*(Parameters|Details)\s*$/i);
    if (match) {
      const variant = match[2].toLowerCase();
      return {
        kind: "tool",
        variant,
        title: `${match[1].trim()} · ${phaseLabel(variant)}`,
        toolish: true,
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
        normalizeChatToolHeading(line).match(/^Command$/i) &&
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
    if (currentSection && fenceTicks == null && isChatAssistantBoundary(line)) {
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
        `<details class="thinking-section"${chatDisplaySettings().expandThinking ? " open" : ""}>` +
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
      const open = variant === "parameters" && chatDisplaySettings().expandParameters;
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
  const pattern = /(`([^`]+)`|\*\*([^*]+)\*\*|__([^_]+)__|\[([^\]\n]+)\]\(([^)\s]+)\))/g;
  let html = "";
  let index = 0;
  for (const match of value.matchAll(pattern)) {
    html += escapeHtml(value.slice(index, match.index));
    if (match[2] !== undefined) {
      html += `<code>${escapeHtml(match[2])}</code>`;
    } else if (match[3] !== undefined || match[4] !== undefined) {
      html += `<strong>${escapeHtml(match[3] ?? match[4])}</strong>`;
    } else if (match[5] !== undefined && match[6] !== undefined) {
      const href = safeMarkdownUrl(match[6]);
      html += href
        ? `<a href="${escapeHtml(href)}" target="_blank" rel="noreferrer noopener">${escapeHtml(match[5])}</a>`
        : escapeHtml(match[0]);
    }
    index = match.index + match[0].length;
  }
  html += escapeHtml(value.slice(index));
  return html;
}

function safeMarkdownUrl(raw) {
  try {
    const url = new URL(raw, window.location.href);
    if (["http:", "https:", "mailto:"].includes(url.protocol)) {
      return url.href;
    }
  } catch {
    return "";
  }
  return "";
}
