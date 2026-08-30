async function loadDbConnections() {
  const body = await api("/api/database/connections");
  state.dbConnections = body.connections || [];
  if (state.selectedDbConnection && !state.dbConnections.some((connection) => connection.id === state.selectedDbConnection)) {
    state.selectedDbConnection = null;
  }
  if (state.selectedDbTargetConnection && !state.dbConnections.some((connection) => connection.id === state.selectedDbTargetConnection)) {
    state.selectedDbTargetConnection = null;
  }
  if (!state.selectedDbConnection && state.dbConnections[0]) {
    state.selectedDbConnection = state.dbConnections[0].id;
  }
  if (!state.selectedDbTargetConnection && state.selectedDbConnection) {
    state.selectedDbTargetConnection = state.selectedDbConnection;
  }
  renderDbConnections();
  renderDbTargetOptions();
}

function renderDbConnections() {
  const target = qs("#db-connections");
  const filter = qs("#db-filter")?.value.trim().toLowerCase() || "";
  const connections = state.dbConnections.filter((connection) => {
    const haystack = [
      connection.name,
      connection.type,
      connection.databaseName,
      connection.filePath,
      connection.host,
    ].join(" ").toLowerCase();
    return !filter || haystack.includes(filter);
  });
  if (!connections.length) {
    target.innerHTML = '<p class="empty">No database connections.</p>';
    qs("#db-explorer-tree").innerHTML = "";
    renderDbTargetOptions();
    return;
  }
  const visible = connections.slice(0, state.limits.dbConnections);
  target.innerHTML = visible
    .map((connection) => `<article class="row ${connection.id === state.selectedDbConnection ? "selected" : ""}">
      <strong>${escapeHtml(connection.name)}</strong>
      <span>${escapeHtml(connection.type)} · ${escapeHtml(connection.databaseName || connection.filePath || connection.host || "")}</span>
      <span class="meta">${escapeHtml([
        connection.host && connection.port ? `:${connection.port}` : "",
        connection.lastTestStatus ? `last test ${connection.lastTestStatus}` : "",
        connection.updatedAt ? `updated ${formatDate(connection.updatedAt)}` : "",
      ].filter(Boolean).join(" · "))}</span>
      <div class="row-actions">
        <button type="button" data-db-id="${connection.id}">Select</button>
        <button type="button" data-db-edit="${connection.id}">Edit</button>
        <button type="button" data-db-test="${connection.id}">Test</button>
        <button type="button" data-db-delete="${connection.id}">Delete</button>
      </div>
    </article>`)
    .join("") + showMoreButton("dbConnections", connections.length, "dbConnections");
  target.querySelectorAll("[data-db-id]").forEach((button) => {
    button.addEventListener("click", () => {
      selectDbConnection(Number(button.dataset.dbId));
    });
  });
  target.querySelectorAll("[data-db-edit]").forEach((button) => {
    button.addEventListener("click", () => editDbConnection(Number(button.dataset.dbEdit)));
  });
  target.querySelectorAll("[data-db-test]").forEach((button) => {
    button.addEventListener("click", () => testDbConnection(Number(button.dataset.dbTest)));
  });
  target.querySelectorAll("[data-db-delete]").forEach((button) => {
    button.addEventListener("click", () => deleteDbConnection(Number(button.dataset.dbDelete)).catch(showError));
  });
  bindShowMore(target);
}

function renderDbTargetOptions() {
  const select = qs("#db-target-connection");
  if (!select) return;
  const previous = select.value || String(state.selectedDbTargetConnection || "");
  select.innerHTML = state.dbConnections
    .map((connection) => `<option value="${connection.id}">${escapeHtml(connection.name)} (${escapeHtml(connection.type)})</option>`)
    .join("");
  if (previous && state.dbConnections.some((connection) => String(connection.id) === previous)) {
    select.value = previous;
  } else if (state.selectedDbConnection) {
    select.value = String(state.selectedDbConnection);
  }
  state.selectedDbTargetConnection = Number(select.value) || null;
}

function selectDbConnection(connectionId) {
  state.selectedDbConnection = connectionId;
  if (!state.selectedDbTargetConnection) {
    state.selectedDbTargetConnection = connectionId;
  }
  const targetSelect = qs("#db-target-connection");
  if (targetSelect && state.selectedDbTargetConnection) {
    targetSelect.value = String(state.selectedDbTargetConnection);
  }
  renderDbConnections();
  setOutput("#db-output", `selected connection ${connectionId}`);
  loadDbExplorer().catch(showError);
}

function selectedDbConnectionProfile() {
  return state.dbConnections.find((connection) => connection.id === selectedDbConnectionId()) || null;
}

async function createDbConnection(event) {
  event.preventDefault();
  const payload = dbConnectionFormPayload();
  if (!payload) return;
  const endpoint = state.editingDbConnection
    ? `/api/database/connections/${encodeURIComponent(state.editingDbConnection)}`
    : "/api/database/connections";
  const body = await api(endpoint, {
    method: state.editingDbConnection ? "PUT" : "POST",
    body: JSON.stringify(payload),
  });
  state.selectedDbConnection = body.connection?.id || state.editingDbConnection || null;
  state.editingDbConnection = null;
  qs("#db-password").value = "";
  qs("#db-save-button").textContent = "Save Connection";
  await loadDbConnections();
  if (state.selectedDbConnection) {
    await loadDbExplorer().catch(showError);
  }
}

function dbConnectionFormPayload() {
  const type = qs("#db-type").value;
  const location = qs("#db-location").value.trim();
  const payload = {
    name: qs("#db-name").value.trim(),
    type,
    databaseName: qs("#db-database").value.trim() || undefined,
    port: qs("#db-port").value ? Number(qs("#db-port").value) : undefined,
    username: qs("#db-username").value.trim() || undefined,
    password: qs("#db-password").value || undefined,
    showAllDatabases: qs("#db-show-all").checked,
  };
  if (!payload.name || !location) return null;
  if (type === "sqlite") {
    payload.filePath = location;
    payload.port = undefined;
    payload.showAllDatabases = false;
  } else {
    payload.host = location;
  }
  return payload;
}

function resetDbConnectionForm() {
  state.editingDbConnection = null;
  qs("#db-name").value = "";
  qs("#db-type").value = "sqlite";
  qs("#db-location").value = "";
  qs("#db-database").value = "";
  qs("#db-username").value = "";
  qs("#db-password").value = "";
  qs("#db-port").value = "";
  qs("#db-show-all").checked = false;
  qs("#db-save-button").textContent = "Save Connection";
}

function editDbConnection(connectionId) {
  const connection = state.dbConnections.find((item) => item.id === connectionId);
  if (!connection) return;
  state.editingDbConnection = connectionId;
  state.selectedDbConnection = connectionId;
  qs("#db-name").value = connection.name || "";
  qs("#db-type").value = connection.type || "sqlite";
  qs("#db-location").value = connection.type === "sqlite"
    ? connection.filePath || ""
    : connection.host || "";
  qs("#db-database").value = connection.databaseName || "";
  qs("#db-username").value = connection.username || "";
  qs("#db-password").value = "";
  qs("#db-port").value = connection.port || "";
  qs("#db-show-all").checked = !!connection.showAllDatabases;
  qs("#db-save-button").textContent = "Update Connection";
  renderDbConnections();
}

async function testDbConnectionForm() {
  const payload = dbConnectionFormPayload();
  if (!payload) return;
  const body = await api("/api/database/connections/test", {
    method: "POST",
    body: JSON.stringify({
      existingConnectionId: state.editingDbConnection || undefined,
      connection: payload,
    }),
  });
  renderJson("#db-output", body);
}

async function testDbConnection(connectionId) {
  const body = await api(`/api/database/connections/${connectionId}/test`, { method: "POST" });
  if (body.connection) {
    const index = state.dbConnections.findIndex((connection) => connection.id === body.connection.id);
    if (index >= 0) state.dbConnections[index] = body.connection;
  }
  renderDbConnections();
  renderJson("#db-output", body);
}

async function deleteDbConnection(connectionId = selectedDbConnectionId()) {
  if (!connectionId) return;
  if (!window.confirm("Delete this database connection?")) return;
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}`, { method: "DELETE" });
  if (state.selectedDbConnection === connectionId) {
    state.selectedDbConnection = null;
    state.selectedDbObject = null;
    qs("#db-explorer-tree").innerHTML = "";
  }
  if (state.selectedDbTargetConnection === connectionId) {
    state.selectedDbTargetConnection = null;
  }
  if (state.editingDbConnection === connectionId) {
    resetDbConnectionForm();
  }
  renderJson("#db-output", body);
  await loadDbConnections();
}

async function runDbQuery(event) {
  event.preventDefault();
  if (!state.selectedDbConnection) return;
  const body = await api(`/api/database/connections/${state.selectedDbConnection}/query`, {
    method: "POST",
    body: JSON.stringify({
      sql: qs("#db-sql").value,
      databaseName: qs("#db-context-database").value.trim() || undefined,
      schemaName: qs("#db-context-schema").value.trim() || undefined,
      maxRows: 200,
    }),
  });
  renderDbResult(body);
}

function selectedDbConnectionId() {
  return state.selectedDbConnection || state.dbConnections[0]?.id || null;
}

async function dbRead(path) {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const body = await api(path.replace("{id}", encodeURIComponent(connectionId)));
  renderJson("#db-output", body);
}

async function loadDbExplorer() {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/explorer`);
  state.dbExplorerNodes = body.nodes || [];
  renderDbExplorer(body, "Database Explorer");
}

async function loadDbExplorerNode(node) {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  setDbObjectContext(node);
  const params = new URLSearchParams({ nodeType: node.type });
  if (node.databaseName) params.set("databaseName", node.databaseName);
  if (node.schemaName) params.set("schemaName", node.schemaName);
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/explorer?${params.toString()}`);
  state.dbExplorerNodes = body.nodes || [];
  renderDbExplorer(body, `${node.type}: ${node.name}`);
}

function renderDbExplorer(body, title) {
  const nodes = body.nodes || [];
  const tree = qs("#db-explorer-tree");
  tree.innerHTML = nodes.length
    ? `<div class="section-label">${escapeHtml(title)}</div>${nodes.map(dbExplorerNodeHtml).join("")}`
    : '<p class="empty">No database objects found.</p>';
  tree.querySelectorAll("[data-db-node]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbNode);
      setDbObjectContext(node);
      if (node.hasChildren) {
        loadDbExplorerNode(node).catch(showError);
        return;
      }
      if (["table", "view"].includes(node.type)) {
        loadDbTableData().catch(showError);
      }
    });
  });
  tree.querySelectorAll("[data-db-details]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbDetails);
      setDbObjectContext(node);
      loadDbObjectDetails().catch(showError);
    });
  });
  tree.querySelectorAll("[data-db-select-sql]").forEach((button) => {
    button.addEventListener("click", () => {
      const node = JSON.parse(button.dataset.dbSelectSql);
      setDbObjectContext(node);
      setDbSql("select");
    });
  });
  renderJson("#db-output", body);
}

function dbExplorerNodeHtml(node) {
  const encoded = escapeHtml(JSON.stringify(node));
  const meta = [
    node.type,
    node.databaseName,
    node.schemaName,
    node.description,
  ].filter(Boolean).join(" · ");
  return `<article class="row db-node">
    <strong>${escapeHtml(node.name)}</strong>
    <span class="meta">${escapeHtml(meta)}</span>
    <div class="row-actions">
      <button type="button" data-db-node="${encoded}">${node.hasChildren ? "Open" : "Use"}</button>
      <button type="button" data-db-details="${encoded}">Details</button>
      ${["table", "view"].includes(node.type) ? `<button type="button" data-db-select-sql="${encoded}">SQL</button>` : ""}
    </div>
  </article>`;
}

function setDbObjectContext(node) {
  state.selectedDbObject = node;
  if (["table", "view"].includes(node.type)) {
    qs("#db-table").value = node.name || "";
  } else {
    qs("#db-table").value = "";
  }
  qs("#db-context-database").value = node.databaseName || "";
  qs("#db-context-schema").value = node.schemaName || "";
  qs("#db-offset").value = "0";
}

function dbContextParams(extra = {}) {
  const params = new URLSearchParams();
  const databaseName = qs("#db-context-database").value.trim();
  const schemaName = qs("#db-context-schema").value.trim();
  if (databaseName) params.set("databaseName", databaseName);
  if (schemaName) params.set("schemaName", schemaName);
  Object.entries(extra).forEach(([key, value]) => {
    if (value !== undefined && value !== null && String(value).trim() !== "") {
      params.set(key, String(value));
    }
  });
  return params;
}

async function loadDbObjectDetails() {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  if (!connectionId) return;
  const objectType = state.selectedDbObject?.type || (tableName ? "table" : "database");
  const objectName = state.selectedDbObject?.name || tableName || qs("#db-context-database").value.trim() || "main";
  const params = dbContextParams({
    objectType,
    name: objectName,
    includeRelational: true,
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/object-details?${params.toString()}`);
  renderDbObjectDetails(body);
}

async function loadDbRelationshipDiagram() {
  const connectionId = selectedDbConnectionId();
  if (!connectionId) return;
  const schemaName = qs("#db-context-schema").value.trim();
  const objectName = schemaName
    || qs("#db-context-database").value.trim()
    || selectedDbConnectionProfile()?.databaseName
    || "main";
  const params = dbContextParams({
    objectType: schemaName ? "schema" : "database",
    name: objectName,
    includeRelational: true,
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/object-details?${params.toString()}`);
  renderDbObjectDetails(body);
}

function renderDbObjectDetails(body) {
  state.lastDbObjectDetails = body;
  const details = body.details || body;
  const columns = details.columns || [];
  const objects = details.objects || [];
  const foreignKeys = details.foreignKeys || [];
  const relationalSchema = details.relationalSchema;
  const relationships = relationalSchema?.relationships || [];
  const diagram = relationalSchema ? renderDbRelationshipDiagram(relationalSchema) : "";
  const target = qs("#db-output");
  target.className = "output-panel result-list";
  target.innerHTML = `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(details.name || "Object Details")}</strong>
      <span class="badge">${escapeHtml(details.type || details.objectType || "table")}</span>
    </header>
    <span class="meta">${escapeHtml([details.databaseName, details.schemaName].filter(Boolean).join(" · "))}</span>
    ${columns.length ? `<div class="table-scroll"><table><thead><tr><th>Column</th><th>Type</th><th>Nullable</th><th>Key</th></tr></thead><tbody>${columns.map((column) => `<tr>
      <td>${escapeHtml(column.name)}</td>
      <td>${escapeHtml(column.dataType || column.nativeType || "")}</td>
      <td>${escapeHtml(column.nullable === false ? "no" : "yes")}</td>
      <td>${column.isPrimaryKey ? "primary" : ""}</td>
    </tr>`).join("")}</tbody></table></div>` : ""}
    ${foreignKeys.length ? `<details open><summary>Foreign keys</summary><div class="table-scroll"><table><thead><tr><th>Column</th><th>References</th><th>Update</th><th>Delete</th></tr></thead><tbody>${foreignKeys.map((key) => `<tr>
      <td>${escapeHtml(key.columnName)}</td>
      <td>${escapeHtml([key.referencedSchemaName, key.referencedTableName, key.referencedColumnName].filter(Boolean).join("."))}</td>
      <td>${escapeHtml(key.onUpdate || "")}</td>
      <td>${escapeHtml(key.onDelete || "")}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${objects.length ? `<details open><summary>Objects</summary><div class="table-scroll"><table><thead><tr><th>Name</th><th>Type</th><th>Database</th><th>Schema</th></tr></thead><tbody>${objects.map((object) => `<tr>
      <td>${escapeHtml(object.name)}</td>
      <td>${escapeHtml(object.type)}</td>
      <td>${escapeHtml(object.databaseName || "")}</td>
      <td>${escapeHtml(object.schemaName || "")}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${diagram}
    ${relationships.length ? `<details><summary>Relationships</summary><div class="table-scroll"><table><thead><tr><th>Source</th><th>Target</th></tr></thead><tbody>${relationships.map((relationship) => `<tr>
      <td>${escapeHtml([relationship.sourceSchemaName, relationship.sourceTableName, relationship.sourceColumnName].filter(Boolean).join("."))}</td>
      <td>${escapeHtml([relationship.targetSchemaName, relationship.targetTableName, relationship.targetColumnName].filter(Boolean).join("."))}</td>
    </tr>`).join("")}</tbody></table></div></details>` : ""}
    ${!columns.length && !objects.length && !foreignKeys.length && !relationships.length ? '<p class="empty">No object details.</p>' : ""}
  </article>`;
  bindDbDiagramControls();
}

function renderDbRelationshipDiagram(schema) {
  const maxTables = 40;
  const tableMap = new Map();
  (schema.tables || []).forEach((table) => {
    tableMap.set(dbTableKey(table.schemaName, table.name), table);
  });
  (schema.relationships || []).forEach((relationship) => {
    const sourceKey = dbTableKey(relationship.sourceSchemaName, relationship.sourceTableName);
    const targetKey = dbTableKey(relationship.targetSchemaName, relationship.targetTableName);
    if (!tableMap.has(sourceKey)) {
      tableMap.set(sourceKey, {
        name: relationship.sourceTableName,
        schemaName: relationship.sourceSchemaName,
        columns: [],
        isExternal: true,
      });
    }
    if (!tableMap.has(targetKey)) {
      tableMap.set(targetKey, {
        name: relationship.targetTableName,
        schemaName: relationship.targetSchemaName,
        columns: [],
        isExternal: true,
      });
    }
  });
  const tables = [...tableMap.values()].slice(0, maxTables);
  if (!tables.length) return "";

  const columns = Math.max(1, Math.ceil(Math.sqrt(tables.length)));
  const nodeWidth = 220;
  const nodeHeight = 148;
  const gapX = 56;
  const gapY = 48;
  const width = columns * nodeWidth + (columns - 1) * gapX + 32;
  const rows = Math.ceil(tables.length / columns);
  const height = rows * nodeHeight + (rows - 1) * gapY + 32;
  const zoom = state.dbDiagram.zoom || 1;
  const query = (state.dbDiagram.query || "").trim().toLowerCase();
  const positions = new Map();
  tables.forEach((table, index) => {
    const col = index % columns;
    const row = Math.floor(index / columns);
    positions.set(dbTableKey(table.schemaName, table.name), {
      x: 16 + col * (nodeWidth + gapX),
      y: 16 + row * (nodeHeight + gapY),
    });
  });

  const paths = (schema.relationships || []).map((relationship, index) => {
    const source = positions.get(dbTableKey(relationship.sourceSchemaName, relationship.sourceTableName));
    const target = positions.get(dbTableKey(relationship.targetSchemaName, relationship.targetTableName));
    if (!source || !target) return "";
    const startX = source.x + nodeWidth;
    const startY = source.y + 44 + (index % 5) * 10;
    const endX = target.x;
    const endY = target.y + 44 + (index % 5) * 10;
    const midX = startX + Math.max(24, (endX - startX) / 2);
    const label = `${relationship.sourceColumnName} -> ${relationship.targetColumnName}`;
    const sourceMatches = tableMatchesDiagramQuery({ schemaName: relationship.sourceSchemaName, name: relationship.sourceTableName }, query);
    const targetMatches = tableMatchesDiagramQuery({ schemaName: relationship.targetSchemaName, name: relationship.targetTableName }, query);
    const dimmed = query && !sourceMatches && !targetMatches ? "dimmed" : "";
    return `<path class="${dimmed}" d="M${startX} ${startY} C${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}" marker-end="url(#db-arrow)" />
      <title>${escapeHtml(label)}</title>`;
  }).join("");

  const nodes = tables.map((table) => {
    const position = positions.get(dbTableKey(table.schemaName, table.name));
    const matches = tableMatchesDiagramQuery(table, query);
    const dimmed = query && !matches ? "dimmed" : "";
    const matched = query && matches ? "matched" : "";
    const columns = (table.columns || []).slice(0, 5);
    const hidden = Math.max(0, (table.columns || []).length - columns.length);
    const columnText = columns.map((column, index) => `<text x="${position.x + 12}" y="${position.y + 58 + index * 18}" class="db-schema-column">
      ${escapeHtml(column.name)}${column.isPrimaryKey ? " *" : ""}${column.dataType ? `: ${escapeHtml(column.dataType)}` : ""}
    </text>`).join("");
    const schemaLabel = [table.schemaName, table.isExternal ? "external" : ""].filter(Boolean).join(" · ");
    return `<g class="db-schema-node ${table.isExternal ? "external" : ""} ${matched} ${dimmed}" data-db-diagram-table="${escapeHtml(table.name)}" data-db-diagram-schema="${escapeHtml(table.schemaName || "")}">
      <rect x="${position.x}" y="${position.y}" width="${nodeWidth}" height="${nodeHeight}" rx="6" />
      <text x="${position.x + 12}" y="${position.y + 24}" class="db-schema-title">${escapeHtml(table.name)}</text>
      ${schemaLabel ? `<text x="${position.x + 12}" y="${position.y + 42}" class="db-schema-meta">${escapeHtml(schemaLabel)}</text>` : ""}
      ${columnText}
      ${hidden ? `<text x="${position.x + 12}" y="${position.y + 58 + columns.length * 18}" class="db-schema-meta">+${hidden} more</text>` : ""}
    </g>`;
  }).join("");

  const clipped = tableMap.size > maxTables
    ? `<p class="empty">Showing ${maxTables} of ${tableMap.size} tables.</p>`
    : "";
  return `<details open class="db-schema-section">
    <summary>Relationship Diagram</summary>
    <div class="db-schema-toolbar">
      <input id="db-diagram-filter" value="${escapeHtml(state.dbDiagram.query)}" placeholder="Filter tables" />
      <button type="button" data-db-diagram-zoom="-0.15">Zoom Out</button>
      <button type="button" data-db-diagram-zoom="0.15">Zoom In</button>
      <button type="button" data-db-diagram-reset>Reset</button>
    </div>
    <div class="db-schema-diagram">
      <svg viewBox="0 0 ${width} ${height}" style="width:${Math.round(width * zoom)}px" role="img" aria-label="Database relationship diagram">
        <defs>
          <marker id="db-arrow" markerWidth="10" markerHeight="10" refX="8" refY="3" orient="auto" markerUnits="strokeWidth">
            <path d="M0,0 L0,6 L9,3 z" />
          </marker>
        </defs>
        <g class="db-schema-links">${paths}</g>
        <g>${nodes}</g>
      </svg>
    </div>
    ${clipped}
  </details>`;
}

function dbTableKey(schemaName, tableName) {
  return `${schemaName || ""}.${tableName || ""}`;
}

function tableMatchesDiagramQuery(table, query) {
  if (!query) return true;
  return [table.name, table.schemaName, table.databaseName]
    .filter(Boolean)
    .join(" ")
    .toLowerCase()
    .includes(query);
}

function bindDbDiagramControls() {
  const root = qs("#db-output");
  root.querySelector("#db-diagram-filter")?.addEventListener("input", (event) => {
    state.dbDiagram.query = event.currentTarget.value;
    renderDbObjectDetails(state.lastDbObjectDetails);
    qs("#db-diagram-filter")?.focus();
  });
  root.querySelectorAll("[data-db-diagram-zoom]").forEach((button) => {
    button.addEventListener("click", () => {
      state.dbDiagram.zoom = Math.min(2.5, Math.max(0.5, state.dbDiagram.zoom + Number(button.dataset.dbDiagramZoom || 0)));
      renderDbObjectDetails(state.lastDbObjectDetails);
    });
  });
  root.querySelector("[data-db-diagram-reset]")?.addEventListener("click", () => {
    state.dbDiagram = { zoom: 1, query: "" };
    renderDbObjectDetails(state.lastDbObjectDetails);
  });
  root.querySelectorAll("[data-db-diagram-table]").forEach((node) => {
    node.addEventListener("click", () => {
      qs("#db-table").value = node.dataset.dbDiagramTable || "";
      qs("#db-context-schema").value = node.dataset.dbDiagramSchema || "";
      state.selectedDbObject = {
        type: "table",
        name: node.dataset.dbDiagramTable || "",
        schemaName: node.dataset.dbDiagramSchema || "",
        databaseName: qs("#db-context-database").value.trim() || undefined,
      };
      loadDbObjectDetails().catch(showError);
    });
  });
}

function setDbSql(kind) {
  const tableName = qs("#db-table").value.trim();
  if (!tableName) return;
  const qualified = dbQualifiedTableName(tableName);
  qs("#db-sql").value = kind === "count"
    ? `SELECT COUNT(*) AS count FROM ${qualified};`
    : `SELECT * FROM ${qualified} LIMIT 100;`;
}

async function loadDbTableData() {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  if (!connectionId || !tableName) return;
  const params = dbContextParams({
    tableName,
    includeTotalCount: true,
    limit: numericInputValue("#db-limit", 50, 1, 500),
    offset: numericInputValue("#db-offset", 0, 0, 1_000_000_000),
  });
  const body = await api(`/api/database/connections/${encodeURIComponent(connectionId)}/table-data?${params.toString()}`);
  renderDbResult(body);
}

async function dbFileJob(path) {
  const connectionId = selectedDbConnectionId();
  const tableName = qs("#db-table").value.trim();
  const filePath = qs("#db-file-path").value.trim();
  if (!connectionId || !tableName || !filePath) return;
  const body = await api(path, {
    method: "POST",
    body: JSON.stringify({
      connectionId,
      tableName,
      databaseName: qs("#db-context-database").value.trim() || undefined,
      schemaName: qs("#db-context-schema").value.trim() || undefined,
      filePath,
    }),
  });
  renderJson("#db-output", body);
}

async function transferDbTable() {
  const sourceId = selectedDbConnectionId();
  const targetId = Number(qs("#db-target-connection")?.value) || sourceId;
  const tableName = qs("#db-table").value.trim();
  if (!sourceId || !tableName) return;
  const targetTable = qs("#db-target-table").value.trim();
  const body = await api("/api/database/transfers", {
    method: "POST",
    body: JSON.stringify({
      mode: qs("#db-transfer-mode").value,
      source: {
        connectionId: sourceId,
        databaseName: qs("#db-context-database").value.trim() || undefined,
        schemaName: qs("#db-context-schema").value.trim() || undefined,
        tableName,
      },
      target: {
        connectionId: targetId,
        databaseName: qs("#db-context-database").value.trim() || undefined,
        schemaName: qs("#db-context-schema").value.trim() || undefined,
        tableName: targetTable || (sourceId === targetId ? `${tableName}_copy` : tableName),
      },
    }),
  });
  renderDbJobs({ jobs: body.job ? [body.job] : [] });
}

async function loadDbJobs() {
  const body = await api("/api/database/jobs");
  renderDbJobs(body);
}

async function loadDbJob(jobId) {
  if (!jobId) return;
  const body = await api(`/api/database/jobs/${encodeURIComponent(jobId)}`);
  renderDbJobs({ jobs: body.job ? [body.job] : [] });
}

function previousDbPage() {
  const limit = numericInputValue("#db-limit", 50, 1, 500);
  const offset = numericInputValue("#db-offset", 0, 0, 1_000_000_000);
  qs("#db-offset").value = String(Math.max(0, offset - limit));
  loadDbTableData().catch(showError);
}

function renderDbResult(body) {
  const result = body.result || body.data || body;
  const rows = result.rows || [];
  const columns = result.columns?.length
    ? result.columns.map((column) => column.name)
    : Object.keys(rows[0] || {});
  const target = qs("#db-output");
  target.className = "output-panel table-output";
  if (!rows.length || !columns.length) {
    renderJson("#db-output", body);
    return;
  }
  const summary = [
    result.tableName,
    result.statementType,
    `${result.returnedRowCount ?? result.rowCount ?? rows.length} rows`,
    result.totalRowCount !== undefined ? `${result.totalRowCount} total` : "",
    result.durationMs !== undefined ? `${result.durationMs} ms` : "",
    result.resultTruncated || result.hasMore ? "truncated" : "",
  ].filter(Boolean).join(" · ");
  const head = columns.map((column) => `<th>${escapeHtml(column)}</th>`).join("");
  const tableRows = rows.map((row) => `<tr>${columns.map((column) => {
    const value = row[column];
    return `<td>${escapeHtml(value === null || value === undefined ? "null" : typeof value === "object" ? JSON.stringify(value) : value)}</td>`;
  }).join("")}</tr>`).join("");
  const pager = result.hasMore ? `<button type="button" data-db-next-page>Next Page</button>` : "";
  const previous = result.offset > 0 ? '<button type="button" data-db-prev-page>Previous Page</button>' : "";
  target.innerHTML = `<div class="output-title"><span>${escapeHtml(summary)}</span><span class="row-actions">${previous}${pager}</span></div><div class="table-scroll"><table><thead><tr>${head}</tr></thead><tbody>${tableRows}</tbody></table></div>`;
  target.querySelector("[data-db-prev-page]")?.addEventListener("click", previousDbPage);
  target.querySelector("[data-db-next-page]")?.addEventListener("click", () => {
    const nextOffset = (result.offset || 0) + (result.limit || rows.length);
    qs("#db-offset").value = String(nextOffset);
    loadDbTableData().catch(showError);
  });
}

function renderDbJobs(body) {
  const jobs = body.jobs || [];
  const target = qs("#db-output");
  target.className = "output-panel result-list";
  if (!jobs.length) {
    target.innerHTML = '<p class="empty">No database jobs.</p>';
    return;
  }
  target.innerHTML = jobs.map((job) => `<article class="result-row">
    <header class="row-title">
      <strong>${escapeHtml(job.type || job.id)}</strong>
      <span class="badge ${job.status === "succeeded" ? "ok" : job.status === "failed" ? "danger" : "warn"}">${escapeHtml(job.status)}</span>
    </header>
    <span class="meta">${escapeHtml(job.id)} · ${escapeHtml(formatDate(job.updatedAt))}</span>
    <span>${escapeHtml([
      job.source?.connectionName || `source ${job.source?.connectionId || ""}`,
      job.source?.tableName,
      "to",
      job.target?.connectionName || `target ${job.target?.connectionId || ""}`,
      job.target?.tableName,
    ].filter(Boolean).join(" "))}</span>
    <button type="button" data-db-job-detail="${escapeHtml(job.id)}">Details</button>
    ${job.error?.message ? `<span>${escapeHtml(job.error.message)}</span>` : ""}
    ${job.logs?.length ? `<details><summary>Logs</summary><pre>${escapeHtml(job.logs.map((log) => `[${formatDate(log.timestamp)}] ${log.level}: ${log.message}`).join("\n"))}</pre></details>` : ""}
  </article>`).join("");
  target.querySelectorAll("[data-db-job-detail]").forEach((button) => {
    button.addEventListener("click", () => loadDbJob(button.dataset.dbJobDetail).catch(showError));
  });
}

function dbQualifiedTableName(tableName) {
  const connection = selectedDbConnectionProfile();
  const type = connection?.type || "sqlite";
  const databaseName = qs("#db-context-database").value.trim();
  const schemaName = qs("#db-context-schema").value.trim();
  const quote = (part) => quoteSqlIdentifier(part, type);
  if (type === "postgresql" && schemaName) {
    return `${quote(schemaName)}.${quote(tableName)}`;
  }
  if ((type === "mysql" || type === "mariadb") && databaseName) {
    return `${quote(databaseName)}.${quote(tableName)}`;
  }
  return quote(tableName);
}

function quoteSqlIdentifier(value, type) {
  const quote = type === "mysql" || type === "mariadb" ? "`" : '"';
  const escaped = String(value).replaceAll(quote, `${quote}${quote}`);
  return `${quote}${escaped}${quote}`;
}

function numericInputValue(selector, fallback, min, max) {
  const value = Number(qs(selector).value);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.trunc(value)));
}
