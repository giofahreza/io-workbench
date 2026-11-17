// Patch only DOM produced by our Markdown renderer (raw HTML is disabled there).
// Keeping the old nodes is important: a stream update must not discard a text
// selection, a focused link, or the user's open/closed disclosure state.
function chatRenderedNodeKey(node) {
  return node.nodeType === 1
    ? node.getAttribute("data-chat-section-key") || node.getAttribute("data-chat-render-key") || ""
    : "";
}

function chatRenderedNodesCompatible(current, next) {
  return current?.nodeType === next.nodeType && (
    next.nodeType !== 1 || (
      current.tagName === next.tagName &&
      chatRenderedNodeKey(current) === chatRenderedNodeKey(next)
    )
  );
}

function patchChatRenderedText(current, value) {
  const previous = current.data;
  if (previous === value) return;
  if (value.startsWith(previous)) {
    current.appendData(value.slice(previous.length));
    return;
  }
  let prefix = 0;
  const length = Math.min(previous.length, value.length);
  while (prefix < length && previous[prefix] === value[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < length - prefix && previous[previous.length - 1 - suffix] === value[value.length - 1 - suffix]) suffix += 1;
  current.replaceData(prefix, previous.length - prefix - suffix, value.slice(prefix, value.length - suffix));
}

function patchChatRenderedNode(current, next) {
  if (current.isEqualNode(next)) return;
  if (current.nodeType === 3) {
    patchChatRenderedText(current, next.data);
    return;
  }
  if (current.nodeType !== 1) return;
  for (const attribute of Array.from(current.attributes)) {
    if (current.tagName === "DETAILS" && attribute.name === "open") continue;
    if (!next.hasAttribute(attribute.name)) current.removeAttribute(attribute.name);
  }
  for (const attribute of Array.from(next.attributes)) {
    if (current.tagName === "DETAILS" && attribute.name === "open") continue;
    if (current.getAttribute(attribute.name) !== attribute.value) current.setAttribute(attribute.name, attribute.value);
  }
  patchChatRenderedChildren(current, next);
}

function patchChatRenderedChildren(parent, nextParent) {
  const desired = Array.from(nextParent.childNodes);
  let cursor = parent.firstChild;
  desired.forEach((next, index) => {
    let candidate = null;
    const key = chatRenderedNodeKey(next);
    // Look ahead for an unchanged block before reusing a compatible one. This
    // preserves existing paragraphs when a new paragraph is inserted above.
    for (let sibling = cursor; sibling; sibling = sibling.nextSibling) {
      if ((key && chatRenderedNodeKey(sibling) === key) || sibling.isEqualNode(next)) {
        candidate = sibling;
        break;
      }
    }
    if (!candidate && chatRenderedNodesCompatible(cursor, next) && !desired.slice(index + 1).some((later) => cursor.isEqualNode(later))) {
      candidate = cursor;
    }
    if (!candidate) {
      candidate = next.cloneNode(true);
      parent.insertBefore(candidate, cursor);
    } else {
      if (candidate !== cursor) parent.insertBefore(candidate, cursor);
      patchChatRenderedNode(candidate, next);
    }
    cursor = candidate.nextSibling;
  });
  while (cursor) {
    const next = cursor.nextSibling;
    cursor.remove();
    cursor = next;
  }
}

function renderChatMessageContent(text, content) {
  if (!text) return;
  const source = String(content || "");
  if (text._iowbHistoryContent === source) return;
  const selection = captureChatRenderedSelection(text);
  const template = document.createElement("template");
  template.innerHTML = renderChatBubbleHtml(source);
  patchChatRenderedChildren(text, template.content);
  restoreChatRenderedSelection(text, selection);
  text._iowbHistoryContent = source;
}

function captureChatRenderedSelection(root) {
  const selection = window.getSelection();
  if (!selection?.rangeCount || (!root.contains(selection.anchorNode) && !root.contains(selection.focusNode))) return null;
  const boundary = (node, offset) => {
    if (!root.contains(node)) return { node, offset };
    const range = document.createRange();
    range.selectNodeContents(root);
    range.setEnd(node, offset);
    return { node, offset, textOffset: range.toString().length };
  };
  return {
    anchor: boundary(selection.anchorNode, selection.anchorOffset),
    focus: boundary(selection.focusNode, selection.focusOffset),
    text: root.textContent,
  };
}

function restoreChatRenderedSelection(root, snapshot) {
  if (!snapshot) return;
  const selection = window.getSelection();
  if (!selection) return;
  if (selection.anchorNode === snapshot.anchor.node && selection.anchorOffset === snapshot.anchor.offset &&
    selection.focusNode === snapshot.focus.node && selection.focusOffset === snapshot.focus.offset) return;
  const previous = snapshot.text;
  const next = root.textContent;
  let prefix = 0;
  const length = Math.min(previous.length, next.length);
  while (prefix < length && previous[prefix] === next[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < length - prefix && previous[previous.length - 1 - suffix] === next[next.length - 1 - suffix]) suffix += 1;
  const boundary = (saved) => {
    if (saved.textOffset === undefined) return saved.node?.isConnected ? saved : null;
    let offset = saved.textOffset;
    if (offset > prefix) {
      offset = offset >= previous.length - suffix
        ? next.length - (previous.length - offset)
        : Math.min(offset, next.length - suffix);
    }
    offset = Math.max(0, Math.min(offset, next.length));
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (offset <= node.length) return { node, offset };
      offset -= node.length;
    }
    return { node: root, offset: root.childNodes.length };
  };
  const anchor = boundary(snapshot.anchor);
  const focus = boundary(snapshot.focus);
  if (!anchor || !focus) return;
  selection.setBaseAndExtent(anchor.node, anchor.offset, focus.node, focus.offset);
}

function chatRenderMessageKey(message) {
  // Local draft IDs are render identities too, but are deliberately not used
  // for persisted-message operations such as Edit from here.
  return String(message?.id || "").trim();
}

function placeChatRenderedLine(output, node, options = {}) {
  if (options.inPlace) {
    options.renderedNodes.push(node);
  } else {
    output.appendChild(node);
  }
}

function reconcileChatMessages(messages, options = {}) {
  const output = options.output || chatOutputRoot();
  if (!output) return;
  const existing = Array.from(output.children).filter((node) => node.dataset.chatHistoryKey);
  const reuseNodes = new Map(existing.map((node) => [node.dataset.chatHistoryKey, node]));
  const renderedNodes = [];
  replayChatMessages(messages, { ...options, output, reuseNodes, inPlace: true, renderedNodes });
  const desired = new Set(renderedNodes);
  if (options.removeMissing !== false) {
    for (const node of existing) if (!desired.has(node)) node.remove();
  }
  // Persisted history precedes live/processing/recovery nodes. Walk backwards
  // so an unchanged transcript receives no remove/append/move operations.
  const lastExisting = existing.filter((node) => node.parentNode === output).at(-1);
  let before = lastExisting ? lastExisting.nextSibling : Array.from(output.children).find((node) => (
    !node.hasAttribute("data-chat-load-older") && !node.dataset.chatHistoryKey
  )) || null;
  for (let index = renderedNodes.length - 1; index >= 0; index -= 1) {
    const node = renderedNodes[index];
    if (node.parentNode !== output || node.nextSibling !== before) output.insertBefore(node, before);
    before = node;
  }
  return renderedNodes;
}
