import { test } from 'node:test';
import assert from 'node:assert/strict';
import { api, info, state } from '../site/preview.js';
import { graphModel } from '../assets/workflow-ui/model.js';

test('public page uses authored examples and preserves parallel branches', async () => {
  const result = await api('/api/state');
  assert.equal(result.run_id, 'public-example');
  assert.equal(result.tasks.filter(task => task.status === 'running').length, 2);
  assert.equal(graphModel(result.tasks).nodes.filter(node => node.status === 'waiting_dependency').length, 2);
  result.tasks.length = 0;
  assert.equal(state.tasks.length, 5);
  assert.equal(info.workspace, 'C:\\Projects\\sample-app');
});
test('public preview cannot dispatch work and can select a task output', async () => {
  await assert.rejects(api('/api/start', { goal: 'work' }), /로컬 앱/);
  const output = await api('/api/activity?stream=example-implementation');
  assert.equal(output.selected_stream.task_id, 'implementation');
  assert.ok(output.selected_stream.entries.length);
  assert.equal(output.selected_stream.can_retry, false);
});
