import { test } from 'node:test';
import assert from 'node:assert/strict';
import { artifactOrder, graphModel, outputChunks, streamsFor, waitReason, usableToolIds } from '../assets/workflow-ui/model.js';

test('independent branches stay ready together and dependencies track actual completion', () => {
  const tasks = [
    { id: 'a', status: 'pending', depends_on: [], role: 'implementation' },
    { id: 'b', status: 'pending', depends_on: [], role: 'review' },
    { id: 'c', status: 'pending', depends_on: ['a', 'b'] },
  ];
  assert.deepEqual(graphModel(tasks).nodes.map(node => node.status), ['ready', 'ready', 'waiting_dependency']);
  tasks[0].status = 'done';
  assert.deepEqual(graphModel(tasks).edges.map(edge => edge.state), ['satisfied', 'pending']);
  tasks[1].status = 'blocked';
  assert.equal(graphModel(tasks).edges[1].state, 'blocked');
});

test('a previous attempt does not hide an active process and waiting reasons remain distinct', () => {
  const activity = { streams: [
    { id: 'old', task_id: 'a', process_alive: false, last_output_at_ms: 200 },
    { id: 'active', task_id: 'a', process_alive: true, last_output_at_ms: 100 },
    { id: 'coordinator', task_id: null, process_alive: true },
  ] };
  assert.equal(streamsFor(activity, 'a')[0].id, 'active');
  assert.equal(streamsFor(activity, null)[0].id, 'coordinator');
  assert.match(waitReason({ wait_reason: { code: 'parallel_limit', limit: 3 } }, []), /3개/);
  assert.match(waitReason({ depends_on: ['upstream'] }, []), /upstream/);
});

test('output uses the backend identity, channel and text without interpreting HTML', () => {
  const chunks = outputChunks({ id: 'attempt-2', truncated: true, entries: [
    { id: 'line-10', source: 'stderr', text: '<script>print("한글")</script>' },
  ] });
  assert.equal(chunks[0].attemptId, 'attempt-2');
  assert.equal(chunks[0].id, 'line-10');
  assert.equal(chunks[0].stream, 'stderr');
  assert.equal(chunks[0].text, '<script>print("한글")</script>\n');
  assert.equal(chunks[0].truncated, true);
});
test('the delivered page precedes evidence pages and implementation files', () => {
  const files = [{ path: 'docs/review.md' }, { path: 'site/evidence/test.html', preview: true },
    { path: 'site/index.html', preview: true }, { path: 'site/app.js' }];
  assert.equal(artifactOrder(files)[0].path, 'site/index.html');
  assert.equal(files[0].path, 'docs/review.md');
});
test('remembered unavailable tools stay saved but are omitted from a new run', () => {
  const saved = new Set(['codex', 'grok']);
  assert.deepEqual(usableToolIds([{ id: 'codex', selectable: true }, { id: 'grok', selectable: false }], saved), ['codex']);
  assert.equal(saved.has('grok'), true);
});
