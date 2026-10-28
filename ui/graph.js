export const GRAPH_PROVENANCE = 'local-svg-fallback';
const W = 220, H = 150, GRID = 16, PAD = 40;
const NS = 'http://www.w3.org/2000/svg';
let graphInstance = 0;
const snap = n => Math.max(PAD, Math.round(n / GRID) * GRID);
const key = e => `${e.from}\u0000${e.to}`;

// Presentation coordinates are independent of execution dependencies.
export function computeLayout(nodes, edges = []) {
  const ids = new Set(nodes.map(n => n.id));
  const deps = new Map(nodes.map(n => [n.id, new Set((n.dependsOn || []).filter(id => ids.has(id)))]));
  for (const e of edges) if (ids.has(e.from) && ids.has(e.to)) deps.get(e.to).add(e.from);
  const ranks = new Map(), visiting = new Set();
  function rank(id) {
    if (ranks.has(id)) return ranks.get(id);
    if (visiting.has(id)) return 0;
    visiting.add(id); let value = 0;
    for (const d of deps.get(id)) value = Math.max(value, rank(d) + 1);
    visiting.delete(id); ranks.set(id, value); return value;
  }
  const columns = [];
  nodes.forEach(n => { const r = rank(n.id); (columns[r] ||= []).push(n.id); });
  // Order each column by its predecessors' rows to reduce crossings.
  for (let r = 1; r < columns.length; r++) {
    const prev = new Map((columns[r - 1] || []).map((id, i) => [id, i]));
    columns[r]?.sort((a, b) => {
      const average = id => { const rows = [...deps.get(id)].filter(d => prev.has(d)).map(d => prev.get(d)); return rows.length ? rows.reduce((x, y) => x + y, 0) / rows.length : Infinity; };
      return average(a) - average(b);
    });
  }
  const positions = {};
  columns.forEach((col, r) => col?.forEach((id, row) => { positions[id] = { x: PAD + r * 320, y: PAD + row * 190 }; }));
  return positions;
}
export function validConnection(from, to, nodes, edges) {
  if (!from || !to || from === to || !nodes.some(n => n.id === from) || !nodes.some(n => n.id === to)) return false;
  if (edges.some(e => e.from === from && e.to === to)) return false;
  const outgoing = new Map(nodes.map(n => [n.id, []]));
  for (const e of edges) outgoing.get(e.from)?.push(e.to);
  const seen = new Set(), stack = [to];
  while (stack.length) { const id = stack.pop(); if (id === from) return false; if (seen.has(id)) continue; seen.add(id); stack.push(...(outgoing.get(id) || [])); }
  return true;
}
export function avoidCollision(position, positions, ignoredId) {
  let p = { x: snap(position.x), y: snap(position.y) };
  const occupied = Object.entries(positions).filter(([id]) => id !== ignoredId).map(([, v]) => v);
  for (let i = 0; i < 1000; i++) {
    if (!occupied.some(q => Math.abs(p.x - q.x) < W + 16 && Math.abs(p.y - q.y) < H + 16)) return p;
    p.y += GRID;
  }
  return p;
}
function path(a, b) {
  const x1 = a.x + W, y1 = a.y + H / 2, x2 = b.x, y2 = b.y + H / 2;
  const c = Math.max(60, Math.abs(x2 - x1) / 2);
  return `M${x1} ${y1} C${x1 + c} ${y1} ${x2 - c} ${y2} ${x2} ${y2}`;
}
function element(doc, tag, cls, text) { const e = doc.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; }
function svgElement(doc, tag, attrs = {}) { const e = doc.createElementNS(NS, tag); for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v); return e; }

export function createGraph(mount, { eventTarget }) {
  const doc = mount.ownerDocument;
  const root = element(doc, 'div', 'wim-graph');
  const toolbar = element(doc, 'div', 'wim-graph__toolbar');
  const summary = element(doc, 'p', 'wim-graph__summary');
  const viewport = element(doc, 'div', 'wim-graph__canvas');
  const stage = element(doc, 'div', 'wim-graph__stage');
  const svg = svgElement(doc, 'svg', { class: 'wim-graph__edges', 'aria-label': '작업 의존 관계' });
  const markerId = `wim-graph-arrow-${++graphInstance}`;
  const defs = svgElement(doc, 'defs');
  const marker = svgElement(doc, 'marker', { id: markerId, viewBox: '0 0 8 8', refX: '7', refY: '4', markerWidth: '8', markerHeight: '8', orient: 'auto', markerUnits: 'userSpaceOnUse' });
  marker.append(svgElement(doc, 'path', { d: 'M0 0 L8 4 L0 8 Z', fill: '#aaa59b' })); defs.append(marker);
  const edgeLayer = svgElement(doc, 'g');
  const ghost = svgElement(doc, 'path', { class: 'wim-graph__ghost' });
  svg.append(defs, edgeLayer, ghost);
  const group = element(doc, 'div', 'wim-graph__nodes');
  stage.append(svg, group); viewport.append(stage);
  const hint = element(doc, 'p', 'wim-graph__hint', '출력 연결점을 끌어 다른 작업의 입력 연결점에 놓으세요. 연결점을 차례로 선택해도 연결할 수 있습니다.');
  const feedback = element(doc, 'p', 'wim-graph__feedback'); feedback.setAttribute('role', 'status');
  const empty = element(doc, 'p', 'wim-graph__empty', '작업자를 추가하거나 계획을 만들면 여기에 표시됩니다.');
  const edgesList = element(doc, 'details', 'wim-graph__edgelist');
  const edgeSummary = element(doc, 'summary', 'wim-graph__edgelist-summary');
  const edgeItems = element(doc, 'ul', 'wim-graph__edgelist-items');
  edgesList.append(edgeSummary, edgeItems);
  root.append(toolbar, summary, hint, feedback, viewport, empty, edgesList); mount.replaceChildren(root);
  mount.dataset.graphProvenance = GRAPH_PROVENANCE;
  let model = {}, nodes = [], edges = [], positions = {}, zoom = 1, scope = null, selected = null, selectedEdge = null, pendingPort = null, drag = null, destroyed = false, autoLayout = true, topology = null, edgeListKey = null;
  const cards = new Map(), wires = new Map();
  const emit = (name, detail = {}) => eventTarget.dispatchEvent(new CustomEvent(`wim:${name}`, { detail }));
  const button = (label, action) => { const b = element(doc, 'button', 'wim-graph__control', label); b.type = 'button'; b.addEventListener('click', action); toolbar.append(b); return b; };
  const add = button('작업자 추가', () => emit('add-worker'));
  const assign = button('이 구성으로 역할 배정', () => emit('assign-roles'));
  const arrange = button('자동 정렬', () => autoArrange());
  button('−', () => setZoom(zoom - .1)).setAttribute('aria-label', '그래프 축소');
  const zoomLabel = element(doc, 'span', 'wim-graph__zoom'); toolbar.append(zoomLabel);
  button('+', () => setZoom(zoom + .1)).setAttribute('aria-label', '그래프 확대');
  button('화면에 맞춤', () => fit());
  const remove = button('선택한 연결 삭제', () => { if (selectedEdge) emit('connect-tasks', { ...selectedEdge, operation: 'remove' }); });
  function persist() {
    try { if (scope) localStorage.setItem(`wim:graph:${scope}`, JSON.stringify({ positions, zoom, autoLayout })); } catch { /* storage is optional */ }
    emit('graph-layout', { positions: structuredClone(positions), zoom });
  }
  function restore() {
    positions = {}; zoom = 1; autoLayout = true; topology = null;
    try { const data = JSON.parse(localStorage.getItem(`wim:graph:${scope}`)); if (data && typeof data.positions === 'object') { positions = data.positions || {}; autoLayout = data.autoLayout !== false; zoom = Math.max(.4, Math.min(1.5, Number(data.zoom) || 1)); } } catch { /* use defaults */ }
  }
  function setZoom(value) { zoom = Math.max(.4, Math.min(1.5, Math.round(value * 10) / 10)); paint(); persist(); }
  function fit() { const width = Math.max(1, stage.offsetWidth); setZoom(Math.min(1, Math.max(.4, (viewport.clientWidth - 20) / width))); }
  function autoArrange() { autoLayout = true; positions = computeLayout(nodes, edges); paint(); persist(); }
  function port(id, side) {
    const b = element(doc, 'button', `wim-graph__port wim-graph__port--${side}`); b.type = 'button'; b.dataset.id = id; b.dataset.side = side;
    b.setAttribute('aria-label', `${id} ${side === 'out' ? '출력' : '입력'} 연결점`); return b;
  }
  function makeCard(n) {
    const card = element(doc, 'div', 'wim-graph__node'); card.dataset.taskId = n.id;
    const head = element(doc, 'button', 'wim-graph__node-head'); head.type = 'button'; head.dataset.taskId = n.id;
    const title = element(doc, 'span', 'wim-graph__title'); const meta = element(doc, 'span', 'wim-graph__meta');
    head.append(title, meta);
    const role = element(doc, 'div', 'wim-graph__role');
    const input = element(doc, 'textarea', 'wim-graph__role-input'); input.rows = 2; input.placeholder = '작업자의 역할을 자연어로 입력'; input.setAttribute('aria-label', `${n.id} 역할`);
    const actions = element(doc, 'div', 'wim-graph__role-actions');
    const save = element(doc, 'button', '', '저장'); save.type = 'button';
    const cancel = element(doc, 'button', '', '취소'); cancel.type = 'button';
    const removeWorker = element(doc, 'button', 'wim-graph__remove-worker', '삭제'); removeWorker.type = 'button';
    removeWorker.setAttribute('aria-label', `${n.id} 작업자 삭제`);
    actions.append(removeWorker, save, cancel); role.append(input, actions);
    const inPort = port(n.id, 'in'), outPort = port(n.id, 'out');
    card.append(head, role, inPort, outPort); group.append(card);
    input.addEventListener('input', () => { card.dataset.editing = 'true'; });
    save.addEventListener('click', () => { const objective = input.value.trim(); input.value = objective; card.dataset.editing = 'true'; card.dataset.submitted = objective; emit('edit-worker', { taskId: n.id, objective }); });
    cancel.addEventListener('click', () => { card.dataset.editing = 'false'; delete card.dataset.submitted; input.value = model.roleHints?.[n.id] || ''; });
    removeWorker.addEventListener('click', () => emit('remove-worker', { taskId: n.id }));
    return { card, head, title, meta, role, input, save, cancel, removeWorker, inPort, outPort };
  }
  function updateCard(n) {
    const c = cards.get(n.id), editable = !!model.canEdit && (model.editableTaskIds || []).includes(n.id);
    c.title.textContent = n.title || n.id; c.meta.textContent = [n.statusLabel, n.difficultyLabel, n.modelLabel].filter(Boolean).join(' · ');
    c.card.dataset.status = n.status || ''; c.head.setAttribute('aria-pressed', String(selected === n.id));
    c.role.hidden = !editable; c.removeWorker.hidden = !(model.removableTaskIds || []).includes(n.id); c.removeWorker.disabled = !!model.busy; c.input.disabled = !!model.busy; c.save.disabled = !!model.busy; c.cancel.disabled = !!model.busy;
    if (c.card.dataset.editing === 'true' && c.input.value.trim() === (model.roleHints?.[n.id] || '')) { c.card.dataset.editing = 'false'; delete c.card.dataset.submitted; }
    if (c.card.dataset.editing !== 'true' && doc.activeElement !== c.input && c.input.value !== (model.roleHints?.[n.id] || '')) { c.input.value = model.roleHints?.[n.id] || ''; c.input.scrollTop = 0; }
    c.inPort.hidden = c.outPort.hidden = !model.canEdit; c.inPort.disabled = !!model.busy || !editable; c.outPort.disabled = !!model.busy; c.inPort.title = editable ? '' : '이 작업은 의존 관계를 수정할 수 없습니다.';
    c.inPort.dataset.valid = String(!!pendingPort && pendingPort.side === 'out' && editable && validConnection(pendingPort.id, n.id, nodes, edges));
    c.outPort.dataset.valid = String(!!pendingPort && pendingPort.side === 'in' && validConnection(n.id, pendingPort.id, nodes, edges));
  }
  function paint() {
    let width = 320, height = 230;
    for (const n of nodes) { const p = positions[n.id]; if (!p) continue; const c = cards.get(n.id); if (!c) continue; c.card.style.left = `${p.x}px`; c.card.style.top = `${p.y}px`; width = Math.max(width, p.x + W + PAD); height = Math.max(height, p.y + H + PAD); }
    stage.style.width = `${width}px`; stage.style.height = `${height}px`; stage.style.transform = `scale(${zoom})`; stage.style.marginRight = `${width * (zoom - 1)}px`; stage.style.marginBottom = `${height * (zoom - 1)}px`;
    svg.setAttribute('width', width); svg.setAttribute('height', height); svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
    for (const e of edges) { const p = wires.get(key(e)); if (p && positions[e.from] && positions[e.to]) { const d = path(positions[e.from], positions[e.to]); p.setAttribute('d', d); p.visual.setAttribute('d', d); } }
    zoomLabel.textContent = `${Math.round(zoom * 100)}%`;
  }
  function refresh() {
    const ids = new Set(nodes.map(n => n.id));
    for (const [id, c] of cards) if (!ids.has(id)) { c.card.remove(); cards.delete(id); delete positions[id]; }
    const defaults = computeLayout(nodes, edges);
    for (const n of nodes) {
      if (!cards.has(n.id)) cards.set(n.id, makeCard(n));
      if (!positions[n.id] || !Number.isFinite(positions[n.id].x) || !Number.isFinite(positions[n.id].y)) positions[n.id] = avoidCollision(defaults[n.id], positions, n.id);
      updateCard(n);
    }
    const edgeKeys = new Set(edges.map(key));
    for (const [k, p] of wires) if (!edgeKeys.has(k)) { p.remove(); p.visual.remove(); wires.delete(k); }
    if (selectedEdge && !edgeKeys.has(key(selectedEdge))) selectedEdge = null;
    const nextEdgeListKey = JSON.stringify(edges.map(key));
    const rebuildEdgeList = nextEdgeListKey !== edgeListKey;
    if (rebuildEdgeList) { edgeItems.replaceChildren(); edgeListKey = nextEdgeListKey; }
    for (const e of edges) {
      let p = wires.get(key(e));
      if (!p) { const visual = svgElement(doc, 'path', { class: 'wim-graph__edge-line' }); p = svgElement(doc, 'path', { class: 'wim-graph__edge', tabindex: '0', role: 'button' }); p.visual = visual; p.dataset.from = e.from; p.dataset.to = e.to; edgeLayer.append(visual, p); wires.set(key(e), p); }
      p.visual.setAttribute('marker-end', `url(#${markerId})`); p.setAttribute('aria-label', `${e.from}에서 ${e.to} 연결 선택`); p.dataset.selected = String(selectedEdge && key(selectedEdge) === key(e)); p.dataset.state = e.state || 'pending'; p.visual.dataset.state = p.dataset.state; p.visual.dataset.selected = p.dataset.selected;
      if (rebuildEdgeList) { const li = element(doc, 'li', 'wim-graph__edgelist-item'); const b = element(doc, 'button', '', `${e.from} → ${e.to}`); b.type = 'button'; b.addEventListener('click', () => selectEdge(e)); li.append(b); edgeItems.append(li); }
    }
    add.disabled = !model.canAdd || !!model.busy; assign.disabled = !model.canEdit || !!model.busy || !nodes.length; arrange.disabled = !nodes.length;
    remove.hidden = !selectedEdge || !model.canEdit; remove.disabled = !!model.busy;
    summary.textContent = `작업 ${nodes.length}개 · 의존 관계 ${edges.length}개`; edgeSummary.textContent = `의존 관계 ${edges.length}개`;
    viewport.hidden = !nodes.length; hint.hidden = !nodes.length || !model.canEdit; empty.hidden = !!nodes.length; edgesList.hidden = !nodes.length;
    paint();
  }
  function selectEdge(e) { selectedEdge = { from: e.from, to: e.to }; refresh(); }
  function connect(a, b) {
    if (!a || !b || a.side === b.side) { feedback.textContent = '출력과 입력 연결점을 차례로 선택하세요.'; return; }
    const from = a.side === 'out' ? a.id : b.id, to = a.side === 'in' ? a.id : b.id;
    if (!(model.editableTaskIds || []).includes(to)) feedback.textContent = '이 작업은 의존 관계를 수정할 수 없습니다. 수정 가능한 작업을 선택하세요.';
    else if (from === to) feedback.textContent = '같은 작업에는 연결할 수 없습니다. 다른 작업을 선택하세요.';
    else if (edges.some(e => e.from === from && e.to === to)) feedback.textContent = '이미 연결된 작업입니다. 다른 연결을 선택하세요.';
    else if (!validConnection(from, to, nodes, edges)) feedback.textContent = '순환하는 연결은 만들 수 없습니다. 다른 작업을 선택하세요.';
    else { feedback.textContent = ''; emit('connect-tasks', { from, to, operation: 'add' }); }
  }
  function canvasPoint(ev) { const rect = stage.getBoundingClientRect(); return { x: (ev.clientX - rect.left) / zoom, y: (ev.clientY - rect.top) / zoom }; }
  root.addEventListener('click', ev => {
    const portEl = ev.target.closest?.('.wim-graph__port');
    if (portEl) { if (drag?.moved) return; const next = { id: portEl.dataset.id, side: portEl.dataset.side }; if (pendingPort) { connect(pendingPort, next); pendingPort = null; } else pendingPort = next; refresh(); return; }
    const edge = ev.target.closest?.('.wim-graph__edge'); if (edge) { selectEdge({ from: edge.dataset.from, to: edge.dataset.to }); return; }
    const head = ev.target.closest?.('.wim-graph__node-head'); if (head && !drag?.moved) { selected = head.dataset.taskId; refresh(); emit('select-task', { taskId: selected, source: 'graph' }); }
  });
  root.addEventListener('keydown', ev => {
    if (ev.key === 'Escape') { pendingPort = null; selectedEdge = null; ghost.setAttribute('d', ''); refresh(); }
    if ((ev.key === 'Delete' || ev.key === 'Backspace') && selectedEdge && model.canEdit && !model.busy && !ev.target.closest('textarea')) { ev.preventDefault(); emit('connect-tasks', { ...selectedEdge, operation: 'remove' }); }
    const focusedEdge = ev.target.closest?.('.wim-graph__edge');
    if ((ev.key === 'Enter' || ev.key === ' ') && focusedEdge) { ev.preventDefault(); selectEdge({ from: focusedEdge.dataset.from, to: focusedEdge.dataset.to }); }
  });
  root.addEventListener('pointerdown', ev => {
    if (ev.button !== 0) return;
    const p = ev.target.closest?.('.wim-graph__port'), head = ev.target.closest?.('.wim-graph__node-head');
    if (p && !p.disabled) { drag = { type: 'wire', id: p.dataset.id, side: p.dataset.side, x: ev.clientX, y: ev.clientY, moved: false }; p.setPointerCapture(ev.pointerId); }
    else if (head) { const id = head.dataset.taskId; drag = { type: 'card', id, x: ev.clientX, y: ev.clientY, origin: { ...positions[id] }, moved: false }; head.setPointerCapture(ev.pointerId); }
  });
  root.addEventListener('pointermove', ev => {
    if (!drag) return;
    if (Math.hypot(ev.clientX - drag.x, ev.clientY - drag.y) > 4) drag.moved = true;
    if (!drag.moved) return;
    if (drag.type === 'card') { positions[drag.id] = { x: Math.max(PAD, drag.origin.x + (ev.clientX - drag.x) / zoom), y: Math.max(PAD, drag.origin.y + (ev.clientY - drag.y) / zoom) }; paint(); }
    else { const point = canvasPoint(ev), p = positions[drag.id]; const a = drag.side === 'out' ? p : { x: point.x - W, y: point.y - H / 2 }; const b = drag.side === 'out' ? { x: point.x, y: point.y - H / 2 } : p; ghost.setAttribute('d', path(a, b)); pendingPort = { id: drag.id, side: drag.side }; for (const n of nodes) updateCard(n); }
  });
  function finish(ev, cancelled = false) {
    if (!drag) return; const d = drag;
    if (d.moved && d.type === 'card') { positions[d.id] = cancelled ? d.origin : avoidCollision(positions[d.id], positions, d.id); if (!cancelled) autoLayout = false; paint(); persist(); }
    if (d.moved && d.type === 'wire') { const target = doc.elementFromPoint(ev.clientX, ev.clientY)?.closest?.('.wim-graph__port'); if (!cancelled && target) connect({ id: d.id, side: d.side }, { id: target.dataset.id, side: target.dataset.side }); else if (!cancelled) feedback.textContent = '입력 또는 출력 연결점 위에 놓으세요.'; pendingPort = null; ghost.setAttribute('d', ''); refresh(); }
    // Click is dispatched after pointerup; retain movement flag through that event.
    if (d.moved) setTimeout(() => { if (drag === d) drag = null; }, 0); else drag = null;
  }
  root.addEventListener('pointerup', ev => finish(ev)); root.addEventListener('pointercancel', ev => finish(ev, true));
  const view = {
    get provenance() { return GRAPH_PROVENANCE; },
    update(next) { if (destroyed) return; model = next || {}; nodes = Array.isArray(model.nodes) ? model.nodes : []; edges = Array.isArray(model.edges) ? model.edges : []; const newScope = model.scope || 'default'; if (scope !== newScope) { for (const c of cards.values()) c.card.remove(); cards.clear(); for (const wire of wires.values()) { wire.visual?.remove(); wire.remove(); } wires.clear(); selected = null; drag = null; topology = null; scope = newScope; restore(); selectedEdge = null; pendingPort = null; ghost.setAttribute('d', ''); } const nextTopology = JSON.stringify([nodes.map(n => [n.id, n.dependsOn || []]), edges.map(key)]); if (autoLayout && topology !== nextTopology && !drag && ![...cards.values()].some(c => c.card.dataset.editing === 'true')) { positions = computeLayout(nodes, edges); topology = nextTopology; } else if (!autoLayout) topology = nextTopology; refresh(); },
    setSelected(id) { selected = id ?? null; for (const n of nodes) updateCard(n); },
    getPositions() { return structuredClone(positions); },
    getRoleDrafts() {
      const drafts = {};
      if (!model.canEdit) return drafts;
      const editable = new Set(model.editableTaskIds || []);
      for (const [id, c] of cards) if (editable.has(id) && c.card.dataset.editing === 'true') drafts[id] = c.input.value.trim();
      return drafts;
    },
    setPositions(value) { autoLayout = false; positions = structuredClone(value || {}); refresh(); persist(); },
    autoArrange,
    destroy() { destroyed = true; mount.replaceChildren(); delete mount.dataset.graphProvenance; },
  };
  view.update({ nodes: [], edges: [] }); return view;
}
