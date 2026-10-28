// Dependency graph — local SVG fallback (module-contract.md §7).
//
// This is the only graph mode. archify is verified only as a Node CLI
// (`archify/bin/archify.mjs validate|render|deliver workflow`, schema v2 JSON →
// standalone HTML). There is no browser-importable module, no live-update
// subscription and no generated artifact under ./vendor/, so this file never
// fetches, imports, probes or frames archify. A future integration keeps the
// GraphView API and follows the steps in contract §7.1.

export const GRAPH_PROVENANCE = 'local-svg-fallback';

const SVG_NS = 'http://www.w3.org/2000/svg';
const NODE_W = 168;
const NODE_H = 56;
const RANK_GAP = 72;
const ROW_GAP = 16;
const STACK_BELOW_PX = 560;
const GROUP_LABEL = '작업 노드 — 화살표 키로 이동, Enter 또는 Space로 선택, Esc로 선택 해제';
const EMPTY_TEXT = '계획이 만들어지면 작업과 의존 관계가 여기에 표시됩니다.';
const EDGE_LABELS = { satisfied: '충족', pending: '대기', blocked: '차단(선행 실패)' };
const SUMMARY_ORDER = ['running', 'routing_hold', 'ready', 'waiting_dependency', 'failed', 'completed'];

let instanceCount = 0;

function el(doc, tag, className) {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  return node;
}

function svgEl(doc, tag, attrs) {
  const node = doc.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs || {})) node.setAttribute(k, String(v));
  return node;
}

function setText(node, value) {
  if (node.textContent !== value) node.textContent = value;
}

function setAttr(node, name, value) {
  if (node.getAttribute(name) !== value) node.setAttribute(name, value);
}

function edgeKey(edge) {
  return `${edge.from}->${edge.to}`;
}

function edgeState(edge) {
  return Object.hasOwn(EDGE_LABELS, edge.state) ? edge.state : 'pending';
}

// Ranks are longest-path from roots; rows follow plan order within a rank.
function computeLayout(nodes, edges) {
  const ids = new Set(nodes.map((n) => n.id));
  const prereqs = new Map(nodes.map((n) => [n.id, new Set()]));
  for (const e of edges) if (ids.has(e.from) && ids.has(e.to)) prereqs.get(e.to).add(e.from);
  for (const n of nodes) for (const d of n.dependsOn || []) if (ids.has(d)) prereqs.get(n.id).add(d);

  const rank = new Map();
  const visiting = new Set();
  const rankOf = (id) => {
    if (rank.has(id)) return rank.get(id);
    if (visiting.has(id)) return 0; // cycle guard: a malformed plan still renders
    visiting.add(id);
    let r = 0;
    for (const p of prereqs.get(id)) r = Math.max(r, rankOf(p) + 1);
    visiting.delete(id);
    rank.set(id, r);
    return r;
  };

  const pos = new Map();
  const byRank = [];
  nodes.forEach((n, planIndex) => {
    const r = rankOf(n.id);
    if (!byRank[r]) byRank[r] = [];
    pos.set(n.id, { rank: r, row: byRank[r].length, planIndex });
    byRank[r].push(n.id);
  });

  const order = [];
  for (const ids of byRank) if (ids) order.push(...ids);
  const rankCount = byRank.length;
  const rowCount = byRank.reduce((m, ids) => Math.max(m, ids ? ids.length : 0), 0);
  return {
    pos,
    byRank,
    order,
    width: rankCount ? rankCount * NODE_W + (rankCount - 1) * RANK_GAP : 0,
    height: rowCount ? rowCount * NODE_H + (rowCount - 1) * ROW_GAP : 0,
  };
}

function nodeX(p) { return p.rank * (NODE_W + RANK_GAP); }
function nodeY(p) { return p.row * (NODE_H + ROW_GAP); }

function edgePath(a, b) {
  const x1 = nodeX(a) + NODE_W;
  const y1 = nodeY(a) + NODE_H / 2;
  const x2 = nodeX(b);
  const y2 = nodeY(b) + NODE_H / 2;
  const c = Math.max(24, (x2 - x1) / 2);
  return `M${x1} ${y1} C${x1 + c} ${y1} ${x2 - c} ${y2} ${x2} ${y2}`;
}

export function createGraph(mount, options) {
  const doc = mount.ownerDocument;
  const eventTarget = options.eventTarget;
  const prefix = `wim-graph-${++instanceCount}`;

  let destroyed = false;
  let nodes = [];
  let edges = [];
  let nodeById = new Map();
  let structure = null;
  let layout = computeLayout([], []);
  let mode = null;
  let selectedId = null;

  const nodeEls = new Map(); // taskId → { button, glyph, title, status, diff, deps, desc }
  const edgeEls = new Map(); // edgeKey → { path, item }

  // ── static DOM ──────────────────────────────────────────────────────────
  const root = el(doc, 'div', 'wim-graph');
  const summary = el(doc, 'p', 'wim-graph__summary');
  const canvas = el(doc, 'div', 'wim-graph__canvas');
  const svg = svgEl(doc, 'svg', { class: 'wim-graph__edges', 'aria-hidden': 'true', focusable: 'false' });
  const defs = svgEl(doc, 'defs');
  const markerId = (state) => `${prefix}-marker-${state}`;
  for (const state of ['satisfied', 'pending']) {
    const marker = svgEl(doc, 'marker', {
      id: markerId(state), class: 'wim-graph__marker wim-graph__marker--arrow', 'data-state': state,
      viewBox: '0 0 8 8', refX: 8, refY: 4, markerWidth: 8, markerHeight: 8,
      markerUnits: 'userSpaceOnUse', orient: 'auto',
    });
    marker.append(svgEl(doc, 'path', { d: 'M0 0 L8 4 L0 8 Z' }));
    defs.append(marker);
  }
  const blockedMarker = svgEl(doc, 'marker', {
    id: markerId('blocked'), class: 'wim-graph__marker wim-graph__marker--blocked', 'data-state': 'blocked',
    viewBox: '0 0 10 10', refX: 10, refY: 5, markerWidth: 10, markerHeight: 10,
    markerUnits: 'userSpaceOnUse', orient: 'auto',
  });
  blockedMarker.append(svgEl(doc, 'path', { d: 'M2 1 L10 9 M10 1 L2 9' }));
  defs.append(blockedMarker);
  const edgeLayer = svgEl(doc, 'g');
  svg.append(defs, edgeLayer);

  const group = el(doc, 'div', 'wim-graph__nodes');
  group.setAttribute('role', 'group');
  group.setAttribute('aria-label', GROUP_LABEL);
  canvas.append(svg, group);

  const descs = el(doc, 'div', 'wim-graph__descs');
  const edgeList = el(doc, 'details', 'wim-graph__edgelist');
  const edgeSummary = el(doc, 'summary', 'wim-graph__edgelist-summary');
  const edgeItems = el(doc, 'ul', 'wim-graph__edgelist-items');
  edgeList.append(edgeSummary, edgeItems);

  const empty = el(doc, 'p', 'wim-graph__empty');
  empty.textContent = EMPTY_TEXT;

  root.append(summary, canvas, descs, edgeList, empty);
  mount.replaceChildren(root);
  mount.dataset.graphProvenance = GRAPH_PROVENANCE;

  // ── layout mode ─────────────────────────────────────────────────────────
  function measureMode() {
    return mount.clientWidth < STACK_BELOW_PX ? 'stacked' : 'ranked';
  }

  function applyLayout() {
    root.dataset.layout = mode;
    if (mode === 'ranked') {
      group.style.width = `${layout.width}px`;
      group.style.height = `${layout.height}px`;
      setAttr(svg, 'width', String(layout.width));
      setAttr(svg, 'height', String(layout.height));
      setAttr(svg, 'viewBox', `0 0 ${layout.width} ${layout.height}`);
      for (const [id, parts] of nodeEls) {
        const p = layout.pos.get(id);
        parts.button.style.left = `${nodeX(p)}px`;
        parts.button.style.top = `${nodeY(p)}px`;
      }
    } else {
      group.style.width = '';
      group.style.height = '';
      for (const parts of nodeEls.values()) {
        parts.button.style.left = '';
        parts.button.style.top = '';
      }
    }
  }

  const resizeObserver = typeof ResizeObserver === 'function'
    ? new ResizeObserver(() => {
      const next = measureMode();
      if (next !== mode) {
        mode = next;
        applyLayout();
      }
    })
    : null;
  resizeObserver?.observe(mount);
  mode = measureMode();
  root.dataset.layout = mode;

  // ── nodes ───────────────────────────────────────────────────────────────
  function createNodeEls(id) {
    const button = el(doc, 'button', 'wim-graph__node');
    button.type = 'button';
    button.dataset.taskId = id;
    button.tabIndex = -1;
    const glyph = el(doc, 'span', 'wim-graph__glyph');
    glyph.setAttribute('aria-hidden', 'true');
    const title = el(doc, 'span', 'wim-graph__title');
    const meta = el(doc, 'span', 'wim-graph__meta');
    const status = el(doc, 'span', 'wim-graph__status');
    const diffPrefix = el(doc, 'span', 'wim-graph__vh');
    diffPrefix.textContent = '역할 ';
    const diff = el(doc, 'span', 'wim-graph__diff');
    const sep = el(doc, 'span', 'wim-graph__sep');
    sep.setAttribute('aria-hidden', 'true');
    sep.textContent = ' · ';
    meta.append(status, sep, diffPrefix, diff);
    const deps = el(doc, 'span', 'wim-graph__deps');
    deps.setAttribute('aria-hidden', 'true'); // the same text is the description
    const desc = el(doc, 'span', 'wim-graph__vh');
    desc.id = `${prefix}-desc-${id}`;
    button.setAttribute('aria-describedby', desc.id);
    button.append(glyph, title, meta, deps);
    descs.append(desc);
    return { button, glyph, title, status, diff, deps, desc };
  }

  function depsText(node) {
    const ids = node.dependsOn || [];
    if (!ids.length) return '선행 작업 없음';
    return `의존: ${ids.map((d) => {
      const dep = nodeById.get(d);
      return dep ? `${d}(${dep.statusLabel})` : d;
    }).join(', ')}`;
  }

  // Why the task is not running yet; dependency waits and routing holds differ.
  function waitText(node) {
    const ids = node.dependsOn || [];
    if (node.status === 'waiting_dependency') {
      const failed = ids.filter((d) => nodeById.get(d)?.status === 'failed');
      if (failed.length) return `${failed.join(', ')} 실패로 대기 중`;
      const open = ids.filter((d) => nodeById.get(d)?.status !== 'completed');
      return open.length ? `${open.join(', ')} 완료를 기다리는 중` : '';
    }
    if (node.status === 'routing_hold') return '의존 충족 · 맞는 모델이 사용 가능해지면 자동 배정 (후보는 작업 카드에 표시)';
    if (node.status === 'ready') return '의존 충족 · 실행 슬롯이 비면 시작';
    return '';
  }

  function refreshNode(node) {
    const parts = nodeEls.get(node.id);
    const { button } = parts;
    if (button.dataset.status !== node.status) button.dataset.status = node.status;
    setText(parts.glyph, node.glyph);
    setText(parts.title, node.title);
    setText(parts.status, node.statusLabel);
    setText(parts.diff, node.difficultyLabel);
    const deps = depsText(node);
    const wait = waitText(node);
    setText(parts.deps, deps);
    setText(parts.desc, wait ? `${deps}. ${wait}` : deps);
    setAttr(button, 'title', [node.title, node.id, deps, wait].filter(Boolean).join('\n'));
    setAttr(button, 'aria-pressed', String(node.id === selectedId));
  }

  function refreshSummary() {
    const counts = new Map();
    const labels = new Map();
    for (const n of nodes) {
      counts.set(n.status, (counts.get(n.status) || 0) + 1);
      if (!labels.has(n.status)) labels.set(n.status, n.statusLabel);
    }
    const parts = SUMMARY_ORDER.filter((s) => counts.has(s)).map((s) => `${labels.get(s)} ${counts.get(s)}`);
    setText(summary, `작업 ${nodes.length}개 · ${parts.join(' · ')}`);
  }

  // ── edges ───────────────────────────────────────────────────────────────
  function refreshEdge(edge) {
    const parts = edgeEls.get(edgeKey(edge));
    const state = edgeState(edge);
    const related = selectedId !== null && (edge.from === selectedId || edge.to === selectedId);
    if (parts.path) {
      setAttr(parts.path, 'data-state', state);
      setAttr(parts.path, 'data-related', String(related));
      setAttr(parts.path, 'marker-end', `url(#${markerId(state)})`);
    }
    setAttr(parts.item, 'data-state', state);
    setText(parts.item, `${edge.from} → ${edge.to} · ${EDGE_LABELS[state]}`);
  }

  // ── structure (node/edge set) ───────────────────────────────────────────
  function structureKey() {
    return JSON.stringify([nodes.map((n) => [n.id, n.dependsOn || []]), edges.map(edgeKey)]);
  }

  function rebuildStructure() {
    const active = doc.activeElement;
    layout = computeLayout(nodes, edges);

    const keep = new Set(nodes.map((n) => n.id));
    for (const [id, parts] of nodeEls) {
      if (!keep.has(id)) {
        parts.button.remove();
        parts.desc.remove();
        nodeEls.delete(id);
      }
    }
    for (const n of nodes) if (!nodeEls.has(n.id)) nodeEls.set(n.id, createNodeEls(n.id));
    // DOM order = rank-then-plan order, so reading and stacked order agree.
    layout.order.forEach((id, i) => {
      const button = nodeEls.get(id).button;
      if (group.children[i] !== button) group.insertBefore(button, group.children[i] || null);
    });

    const edgeKeys = new Set(edges.map(edgeKey));
    for (const [key, parts] of edgeEls) {
      if (!edgeKeys.has(key)) {
        parts.path?.remove();
        parts.item.remove();
        edgeEls.delete(key);
      }
    }
    edges.forEach((edge, i) => {
      const key = edgeKey(edge);
      let parts = edgeEls.get(key);
      if (!parts) {
        const item = el(doc, 'li', 'wim-graph__edgelist-item');
        item.dataset.from = edge.from;
        item.dataset.to = edge.to;
        parts = { path: null, item };
        edgeEls.set(key, parts);
      }
      const a = layout.pos.get(edge.from);
      const b = layout.pos.get(edge.to);
      if (a && b) {
        if (!parts.path) {
          parts.path = svgEl(doc, 'path', { class: 'wim-graph__edge', 'data-from': edge.from, 'data-to': edge.to });
          edgeLayer.append(parts.path);
        }
        parts.path.setAttribute('d', edgePath(a, b));
      } else {
        parts.path?.remove();
        parts.path = null;
      }
      if (edgeItems.children[i] !== parts.item) edgeItems.insertBefore(parts.item, edgeItems.children[i] || null);
    });

    applyLayout();
    if (active && active !== doc.activeElement && root.contains(active)) active.focus({ preventScroll: true });
  }

  // ── roving tab stop and selection ───────────────────────────────────────
  function applyTabStop() {
    const active = doc.activeElement;
    const focusedInside = active && group.contains(active) && active.classList.contains('wim-graph__node');
    const target = focusedInside
      ? active.dataset.taskId
      : (nodeById.has(selectedId) ? selectedId : layout.order[0]);
    setRoving(target);
  }

  function refreshSelection() {
    for (const n of nodes) setAttr(nodeEls.get(n.id).button, 'aria-pressed', String(n.id === selectedId));
    for (const e of edges) refreshEdge(e);
    applyTabStop();
  }

  function setRoving(id) {
    for (const [other, p] of nodeEls) {
      const value = other === id ? 0 : -1;
      if (p.button.tabIndex !== value) p.button.tabIndex = value;
    }
  }

  function focusNode(id) {
    const parts = nodeEls.get(id);
    if (!parts) return;
    setRoving(id);
    parts.button.focus();
  }

  function dispatchSelect(taskId) {
    eventTarget.dispatchEvent(new CustomEvent('wim:select-task', {
      detail: { taskId, source: 'graph' },
      bubbles: false,
    }));
  }

  // ── listeners ───────────────────────────────────────────────────────────
  function onClick(ev) {
    const button = ev.target.closest?.('.wim-graph__node');
    if (!button || !group.contains(button)) return;
    const id = button.dataset.taskId;
    selectedId = id;
    refreshSelection();
    dispatchSelect(id);
  }

  function nextNodeId(id, key) {
    const order = layout.order;
    const i = order.indexOf(id);
    if (key === 'Home') return order[0];
    if (key === 'End') return order[order.length - 1];
    if (mode === 'stacked') {
      if (key === 'ArrowUp' || key === 'ArrowLeft') return order[i - 1] ?? null;
      if (key === 'ArrowDown' || key === 'ArrowRight') return order[i + 1] ?? null;
      return null;
    }
    const p = layout.pos.get(id);
    const column = layout.byRank[p.rank];
    if (key === 'ArrowUp') return column[p.row - 1] ?? null;
    if (key === 'ArrowDown') return column[p.row + 1] ?? null;
    const target = layout.byRank[p.rank + (key === 'ArrowRight' ? 1 : -1)];
    if (!target || !target.length) return null;
    return target[Math.min(p.row, target.length - 1)]; // nearest row
  }

  function onKeydown(ev) {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      selectedId = null;
      refreshSelection();
      dispatchSelect(null);
      return;
    }
    if (ev.altKey || ev.ctrlKey || ev.metaKey) return;
    if (!['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(ev.key)) return;
    const button = ev.target.closest?.('.wim-graph__node');
    if (!button || !group.contains(button)) return;
    ev.preventDefault();
    const next = nextNodeId(button.dataset.taskId, ev.key);
    if (next) focusNode(next); // arrows move focus only, never selection
  }

  function onFocusIn(ev) {
    const button = ev.target.closest?.('.wim-graph__node');
    if (button) setRoving(button.dataset.taskId);
  }

  function onFocusOut(ev) {
    if (!group.contains(ev.relatedTarget)) {
      // Leaving the group: the single tab stop returns to selected-or-first.
      queueMicrotask(() => { if (!destroyed) applyTabStop(); });
    }
  }

  group.addEventListener('click', onClick);
  root.addEventListener('keydown', onKeydown);
  group.addEventListener('focusin', onFocusIn);
  group.addEventListener('focusout', onFocusOut);

  // ── public API ──────────────────────────────────────────────────────────
  const view = {
    get provenance() { return GRAPH_PROVENANCE; },

    update(model) {
      if (destroyed) return;
      nodes = Array.isArray(model?.nodes) ? model.nodes : [];
      edges = Array.isArray(model?.edges) ? model.edges : [];
      nodeById = new Map(nodes.map((n) => [n.id, n]));

      const isEmpty = nodes.length === 0;
      root.dataset.empty = String(isEmpty);
      empty.hidden = !isEmpty;
      summary.hidden = isEmpty;
      canvas.hidden = isEmpty;
      edgeList.hidden = isEmpty;

      const key = structureKey();
      if (key !== structure) {
        structure = key;
        rebuildStructure();
      }
      for (const n of nodes) refreshNode(n);
      for (const e of edges) refreshEdge(e);
      setText(edgeSummary, `의존 관계 ${edges.length}개`);
      refreshSummary();
      applyTabStop();
    },

    setSelected(taskId) {
      if (destroyed) return;
      selectedId = taskId ?? null;
      refreshSelection();
    },

    destroy() {
      if (destroyed) return;
      destroyed = true;
      resizeObserver?.disconnect();
      group.removeEventListener('click', onClick);
      root.removeEventListener('keydown', onKeydown);
      group.removeEventListener('focusin', onFocusIn);
      group.removeEventListener('focusout', onFocusOut);
      nodeEls.clear();
      edgeEls.clear();
      mount.replaceChildren();
      delete mount.dataset.graphProvenance;
    },
  };

  view.update({ nodes: [], edges: [] });
  return view;
}
