// Dependency-free browser qualification. Uses an isolated, owned Chromium profile.
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const [browser, url = 'http://127.0.0.1:58432/'] = process.argv.slice(2);
if (!browser) throw Error('Usage: node tests/capture_workflow_ui.mjs ABS_CHROMIUM_EXE [LOCAL_URL]');
const out = resolve('.impeccable/review');
const profile = resolve('.switchbite/ui-browser/profile-' + Date.now());
await mkdir(out, { recursive: true }); await mkdir(profile, { recursive: true });
const child = spawn(browser, ['--headless=new', '--no-first-run', '--no-default-browser-check',
  '--disable-background-networking', '--disable-extensions', '--remote-debugging-port=0',
  '--remote-debugging-address=127.0.0.1', '--user-data-dir=' + profile, 'about:blank'],
{ windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] });
let diagnostics = ''; child.stderr.on('data', data => { diagnostics = (diagnostics + data).slice(-4000); });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
let socket;
try {
  let endpoint;
  for (let i = 0; i < 100; i++) {
    try { const [port, path] = (await readFile(profile + '/DevToolsActivePort', 'utf8')).trim().split('\n'); endpoint = `ws://127.0.0.1:${port}${path}`; break; } catch {}
    if (child.exitCode != null) throw Error('Browser exited: ' + diagnostics);
    await pause(100);
  }
  if (!endpoint) throw Error('Browser did not become ready');
  socket = new WebSocket(endpoint); await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0; const pending = new Map(), exceptions = [];
  socket.onmessage = message => {
    const event = JSON.parse(message.data);
    if (event.method === 'Runtime.exceptionThrown') exceptions.push(event.params.exceptionDetails.exception?.description || event.params.exceptionDetails.text);
    if (event.id && pending.has(event.id)) { const entry = pending.get(event.id); pending.delete(event.id); clearTimeout(entry.timer); if (event.error) entry.reject(Error(JSON.stringify(event.error))); else entry.resolve(event.result); }
  };
  function send(method, params = {}, sessionId) {
    return new Promise((resolve, reject) => {
      const id = ++sequence, timer = setTimeout(() => { pending.delete(id); reject(Error('CDP timeout: ' + method)); }, 15000);
      pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
  const command = (method, params = {}) => send(method, params, sessionId);
  const evaluate = async expression => {
    const result = await command('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw Error(result.exceptionDetails.text + ': ' + result.exceptionDetails.exception?.description);
    return result.result.value;
  };
  const until = async expression => { for (let i = 0; i < 100; i++) { if (await evaluate(expression)) return; await pause(100); } throw Error('UI did not settle: ' + expression); };
  await command('Page.enable'); await command('Runtime.enable');
  await command('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  const proof = { url, captures: [], runtime_errors: exceptions };
  for (const [name, width, height] of [['desktop', 1440, 900], ['mobile', 375, 900]]) {
    await command('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: name === 'mobile' });
    await command('Page.navigate', { url });
    await until("document.querySelector('#wim-connection')?.textContent==='Live' && document.querySelectorAll('.wim-card').length>0");
    await evaluate("document.fonts.ready");
    await evaluate("document.querySelector('.wim-card[data-task-id=\"build-demo-runtime\"] .wim-card__toggle')?.click()");
    await until("document.querySelector('.wim-card[data-task-id=\"build-demo-runtime\"] .wim-out__pre')?.textContent.length>0");
    await evaluate('scrollTo(0,0)');
    const metrics = await evaluate("({width:innerWidth,bodyWidth:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight,cards:document.querySelectorAll('.wim-card').length,graphNodes:document.querySelectorAll('.wim-graph__node').length,output:document.querySelector('.wim-card[data-task-id=\"build-demo-runtime\"] .wim-out__pre').textContent.length,previewButtons:document.querySelectorAll('[data-artifact-path] button').length})");
    if (metrics.bodyWidth > width + 1) throw Error(`${name}: page overflows (${metrics.bodyWidth}>${width})`);
    if (metrics.cards !== metrics.graphNodes) throw Error('Graph/card count mismatch');
    const screenshot = await command('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true,
      clip: { x: 0, y: 0, width, height: metrics.height, scale: 1 } });
    const path = resolve(out, name + '.png'); await writeFile(path, Buffer.from(screenshot.data, 'base64'));
    proof.captures.push({ name, path, ...metrics });
  }
  // Verify creation can be opened without starting an AI task, and invalid paths preserve input.
  await command('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await evaluate("document.querySelector('#wim-btn-reset').click()");
  await until("!document.querySelector('#wim-start-form').hidden");
  const original = await evaluate("({workspace:document.querySelector('#wim-input-workspace').value, goal:document.querySelector('#wim-input-goal').value,coordinators:document.querySelector('#wim-input-coordinator').options.length,selected:document.querySelectorAll('#wim-tools-list input:checked').length})");
  if (original.goal !== '' || !original.coordinators || !original.selected) throw Error('New task form is not connected to detected tools');
  const missing = resolve('.switchbite/ui-browser/nonexistent-project-' + Date.now());
  await evaluate(`document.querySelector('#wim-input-workspace').value=${JSON.stringify(missing)}; document.querySelector('#wim-input-goal').value='화면 연결 검사'; document.querySelector('#wim-input-goal').dispatchEvent(new Event('input')); document.querySelector('#wim-start-form').requestSubmit();`);
  await until("!document.querySelector('#wim-btn-start').disabled && document.querySelector('#wim-start-error').textContent.length>0");
  proof.invalid_submission = await evaluate("({message:document.querySelector('#wim-start-error').textContent,goal:document.querySelector('#wim-input-goal').value,workspace:document.querySelector('#wim-input-workspace').value})");
  if (proof.invalid_submission.goal !== '화면 연결 검사') throw Error('Failed submission lost input');
  await evaluate('scrollTo(0,0)');
  const formShot = await command('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(resolve(out, 'desktop-form.png'), Buffer.from(formShot.data, 'base64'));
  if (exceptions.length) throw Error('Browser exceptions: ' + exceptions.join('\n'));
  await writeFile(resolve(out, 'browser-proof.json'), JSON.stringify(proof, null, 2));
  console.log(JSON.stringify(proof, null, 2));
  await send('Browser.close');
} finally {
  socket?.close();
  if (child.exitCode == null) child.kill();
}
