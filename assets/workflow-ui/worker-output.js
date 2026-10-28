// Worker output view (module-contract.md §8).
//
// Chunk text is written only through textContent and never trimmed, normalized
// or interpreted. stream, tick and truncated are metadata: they live in data-*
// attributes and the separate metadata list, never inside the text.
// Invariant: pre.textContent === getText() === synced chunk texts joined.

const FOLLOW_STOP_PX = 24;
const FOLLOW_RESUME_PX = 4;
const EMPTY_TEXT = '아직 출력이 없습니다. 실행이 시작되면 받은 원문을 그대로 표시합니다.';
const COPY_OK = '복사했습니다';
const COPY_FAIL = '복사하지 못했습니다. 텍스트를 직접 선택해 복사하세요.';

const encoder = new TextEncoder();

function el(doc, tag, className, text) {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function virtualClock(tick) {
  const t = Math.max(0, Number(tick) || 0);
  return `${String(Math.floor(t / 60)).padStart(2, '0')}:${String(t % 60).padStart(2, '0')}`;
}

export function createOutputView(mount, options) {
  const { attemptId, label, eventTarget } = options;
  const doc = mount.ownerDocument;

  let destroyed = false;
  let cursor = 0;          // indexes of the input array consumed so far
  let lastSeenId = null;   // id at cursor − 1, to detect a replaced array
  let parts = [];          // accepted chunk texts, verbatim
  let stderrCount = 0;
  let truncatedCount = 0;
  let following = true;
  let unseen = 0;
  let expectedTop = null;  // scrollTop we set ourselves; that scroll event is ignored

  // ── DOM ─────────────────────────────────────────────────────────────────
  const section = el(doc, 'section', 'wim-out');
  section.setAttribute('aria-label', `${label} 출력`);
  section.dataset.attemptId = attemptId;

  const bar = el(doc, 'div', 'wim-out__bar');
  const meta = el(doc, 'span', 'wim-out__meta');
  const windowNote = el(doc, 'span', 'wim-out__window');
  const followBtn = el(doc, 'button', 'wim-out__follow', '최신 출력 따라가기');
  followBtn.type = 'button';
  const copyBtn = el(doc, 'button', 'wim-out__copy', '출력 복사');
  copyBtn.type = 'button';
  const status = el(doc, 'span', 'wim-out__status');
  status.setAttribute('role', 'status');
  bar.append(meta, windowNote, followBtn, copyBtn, status);

  const pre = el(doc, 'pre', 'wim-out__pre');
  pre.tabIndex = 0;
  pre.setAttribute('role', 'region');
  pre.setAttribute('aria-label', `${label} 원문 출력 · ${attemptId}`);

  const jump = el(doc, 'button', 'wim-out__jump');
  jump.type = 'button';
  jump.hidden = true;

  const details = el(doc, 'details', 'wim-out__details');
  const detailsSummary = el(doc, 'summary', 'wim-out__details-summary');
  const chunkList = el(doc, 'ol', 'wim-out__chunklist');
  details.append(detailsSummary, chunkList);

  const empty = el(doc, 'p', 'wim-out__empty', EMPTY_TEXT);

  section.append(bar, pre, jump, details, empty);
  mount.append(section);

  // ── rendering ───────────────────────────────────────────────────────────
  function renderCounts() {
    const n = parts.length;
    meta.textContent = `${attemptId} · 조각 ${n}개 · stderr ${stderrCount}개 · ┆ 표시 = stderr`;
    windowNote.textContent = truncatedCount
      ? `전체 표시 · 백엔드가 잘라 보낸 조각 ${truncatedCount}개 (받은 그대로 표시)`
      : '전체 표시 · 잘림 없음';
    detailsSummary.textContent = `조각 메타데이터 ${n}개 (시각·스트림)`;
    pre.hidden = n === 0;
    details.hidden = n === 0;
    empty.hidden = n !== 0;
  }

  function renderFollow() {
    followBtn.setAttribute('aria-pressed', String(following));
    section.dataset.following = String(following);
    jump.hidden = following || unseen === 0;
    jump.textContent = `새 출력 ${unseen}개 — 맨 아래로`;
  }

  function appendChunk(chunk) {
    const text = chunk.text;
    const span = el(doc, 'span', 'wim-out__chunk');
    span.dataset.chunkId = chunk.id;
    span.dataset.stream = chunk.stream;
    if (chunk.truncated === true) span.dataset.truncated = 'true';
    span.textContent = text; // empty chunks still get a span
    pre.append(span);
    parts.push(text);
    if (chunk.stream === 'stderr') stderrCount += 1;
    if (chunk.truncated === true) truncatedCount += 1;

    const item = el(doc, 'li', 'wim-out__chunkmeta');
    item.dataset.chunkId = chunk.id;
    item.dataset.stream = chunk.stream;
    const flags = [];
    if (text === '') flags.push('빈 조각');
    if (chunk.truncated === true) flags.push('백엔드에서 잘림');
    item.textContent = [
      `#${chunk.seq}`,
      chunk.stream,
      chunk.timestamp ? new Date(chunk.timestamp).toLocaleTimeString('ko-KR') : '기록 시각 없음',
      `${encoder.encode(text).length}바이트`,
      ...flags,
    ].join(' · ');
    chunkList.append(item);
  }

  function clearAll() {
    pre.replaceChildren();
    chunkList.replaceChildren();
    parts = [];
    stderrCount = 0;
    truncatedCount = 0;
    cursor = 0;
    lastSeenId = null;
  }

  // ── follow ──────────────────────────────────────────────────────────────
  function selectionInPre() {
    const sel = doc.getSelection?.();
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return false;
    return pre.contains(sel.anchorNode) || pre.contains(sel.focusNode);
  }

  function scrollToBottom() {
    // Direct assignment, never smooth, so reduced motion needs no branch.
    pre.scrollTop = pre.scrollHeight;
    expectedTop = pre.scrollTop;
  }

  function emitFollow() {
    eventTarget.dispatchEvent(new CustomEvent('wim:follow-change', {
      detail: { attemptId, following, unseen },
      bubbles: false,
    }));
  }

  function setFollowing(on) {
    const changed = on !== following || (on && unseen !== 0);
    following = on;
    if (on) {
      unseen = 0;
      scrollToBottom();
    }
    renderFollow();
    if (changed) emitFollow();
  }

  function onScroll() {
    if (expectedTop !== null && Math.abs(pre.scrollTop - expectedTop) < 1) {
      expectedTop = null;
      return;
    }
    expectedTop = null;
    const below = pre.scrollHeight - pre.clientHeight - pre.scrollTop;
    if (following && below > FOLLOW_STOP_PX) setFollowing(false);
    else if (!following && below <= FOLLOW_RESUME_PX) setFollowing(true);
  }

  // A collapsed card or hidden attempt tab has no scroll height; when it is
  // shown again, a following view returns to the newest output.
  const resizeObserver = typeof ResizeObserver === 'function'
    ? new ResizeObserver(() => { if (following && !selectionInPre()) scrollToBottom(); })
    : null;
  resizeObserver?.observe(pre);

  function onFollowClick() { setFollowing(!following); }
  function onCopyClick() { view.copy(); }
  function onJumpClick() {
    setFollowing(true);
    pre.focus({ preventScroll: true }); // the jump button hides itself
  }

  pre.addEventListener('scroll', onScroll, { passive: true });
  followBtn.addEventListener('click', onFollowClick);
  copyBtn.addEventListener('click', onCopyClick);
  jump.addEventListener('click', onJumpClick);

  // ── public API ──────────────────────────────────────────────────────────
  const view = {
    get attemptId() { return attemptId; },

    sync(chunks) {
      if (destroyed) return;
      if (!Array.isArray(chunks)) {
        console.error(`[worker-output] ${attemptId}: sync expects an array`, chunks);
        return;
      }
      if (chunks.length < cursor || (cursor > 0 && chunks[cursor - 1]?.id !== lastSeenId)
        || chunks.slice(0, cursor).some((chunk, index) => chunk.text !== parts[index])) {
        if (selectionInPre()) return;
        clearAll();
      }
      const before = parts.length;
      for (; cursor < chunks.length; cursor += 1) {
        const chunk = chunks[cursor];
        lastSeenId = chunk?.id ?? null;
        if (!chunk || chunk.attemptId !== attemptId) {
          console.error(`[worker-output] ${attemptId}: skipped chunk of another attempt`, chunk);
          continue;
        }
        if (typeof chunk.text !== 'string') {
          console.error(`[worker-output] ${attemptId}: skipped chunk without string text`, chunk);
          continue;
        }
        appendChunk(chunk);
      }
      const appended = parts.length - before;
      renderCounts();
      if (appended <= 0) return;
      if (following) {
        if (!selectionInPre()) scrollToBottom();
      } else {
        unseen += appended;
        renderFollow();
        emitFollow();
      }
    },

    getText() {
      return parts.join('');
    },

    async copy() {
      const text = parts.join('');
      let ok = false;
      try {
        if (navigator.clipboard?.writeText) {
          await navigator.clipboard.writeText(text);
          ok = true;
        }
      } catch {
        ok = false;
      }
      if (!ok && !destroyed) {
        const sel = doc.getSelection?.();
        if (sel) {
          const range = doc.createRange();
          range.selectNodeContents(pre);
          sel.removeAllRanges();
          sel.addRange(range);
        }
      }
      status.textContent = ok ? COPY_OK : COPY_FAIL;
      eventTarget.dispatchEvent(new CustomEvent('wim:output-copy', {
        detail: { attemptId, ok, length: text.length },
        bubbles: false,
      }));
      return ok;
    },

    setFollow(on) {
      if (destroyed) return;
      setFollowing(Boolean(on));
    },

    isFollowing() {
      return following;
    },

    destroy() {
      if (destroyed) return;
      destroyed = true;
      resizeObserver?.disconnect();
      pre.removeEventListener('scroll', onScroll);
      followBtn.removeEventListener('click', onFollowClick);
      copyBtn.removeEventListener('click', onCopyClick);
      jump.removeEventListener('click', onJumpClick);
      section.remove();
    },
  };

  renderCounts();
  renderFollow();
  return view;
}
