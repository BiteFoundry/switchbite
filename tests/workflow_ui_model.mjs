import { test } from 'node:test';
import assert from 'node:assert/strict';
import { artifactOrder, graphModel, outputChunks, streamsFor, waitReason, usableToolIds, failureText, feedbackView } from '../assets/workflow-ui/model.js';

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
test('failure identifies the stage and field without exposing the previous plan', () => {
  const text = failureText({ stage: 'plan_validation', node_id: 'audit', field: 'routing.model', message: '모델을 확인하세요.', expected: 'string', prior_result: { secret: 'private-plan' } });
  assert.match(text, /계획 검증/);
  assert.match(text, /audit · routing.model/);
  assert.match(text, /기대 형식: string/);
  assert.ok(!text.includes('private-plan'));
});
test('feedback distinguishes a queued report from the coordinator decision and plan changes', () => {
  const pending = feedbackView({ id: 'one', source: 'worker', task_id: 'api', status: 'pending', message: '依存先が変更されました' });
  assert.equal(pending.status, '조율 대기');
  assert.match(pending.source, /api/);
  assert.equal(pending.message, '依存先が変更されました');
  const handled = feedbackView({ source: 'user', status: 'processed', reply: '수정 작업을 추가했습니다.', changes: { added: ['fix'], changed: ['review'], removed: [] } });
  assert.equal(handled.status, '답변 완료');
  assert.equal(handled.changes, '추가: fix · 변경: review');
});
