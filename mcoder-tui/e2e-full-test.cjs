// 端到端测试：基础对话 + 工具调用 + 工作流
const WebSocket = require('ws');

const URL = 'ws://127.0.0.1:7654';
const TOKEN = '8263ea1f8d4142b780a761535abdef7d';
const PROJECT = process.cwd();

let msgId = 1;
const nextId = () => msgId++;

function send(ws, method, params) {
  const id = nextId();
  ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  return id;
}

class TestRunner {
  constructor() {
    this.ws = null;
    this.sessionId = null;
    this.results = { pass: 0, fail: 0, tests: [] };
    this.currentTest = null;
    this.responseText = '';
    this.toolCalls = [];
    this.done = false;
    this.doneReason = '';
  }

  log(msg) { console.log(`  ${msg}`); }
  pass(name) { this.results.pass++; this.results.tests.push({ name, status: 'pass' }); console.log(`  [PASS] ${name}`); }
  fail(name, reason) { this.results.fail++; this.results.tests.push({ name, status: 'fail', reason }); console.log(`  [FAIL] ${name}: ${reason}`); }

  async connect() {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(URL);
      this.ws.on('open', () => {
        this.log('connected, sending auth...');
        send(this.ws, 'auth', { token: TOKEN });
      });
      this.ws.on('message', (data) => this.handleMessage(data, resolve));
      this.ws.on('error', reject);
    });
  }

  handleMessage(data, resolve) {
    const msg = JSON.parse(data.toString());
    const method = msg.method;

    // RPC 响应
    if (msg.id !== undefined && msg.result !== undefined) {
      if (msg.id === 0 || msg.id === 1) {
        this.log('auth ok');
        send(this.ws, 'sessions.create', { title: 'e2e-full', project: PROJECT });
      } else if (msg.id === 2) {
        this.sessionId = msg.result?.session_id;
        this.log(`session created: ${this.sessionId}`);
        send(this.ws, 'session.attach', { session_id: this.sessionId });
      } else if (msg.id === 3) {
        this.log('attached');
        resolve();
      } else if (msg.id >= 4) {
        // 后续 send 的响应
      }
      return;
    }

    if (msg.id !== undefined && msg.error) {
      console.error('  [rpc error]', JSON.stringify(msg.error));
      return;
    }

    // 通知
    if (method === 'message') {
      const params = msg.params || {};
      const message = params.message || {};
      const blocks = message.content || [];
      for (const block of blocks) {
        if (block.type === 'text' && block.text) {
          this.responseText += block.text;
          this.log(`[assistant] ${block.text.slice(0, 150)}`);
        }
        if (block.type === 'tool_use') {
          this.toolCalls.push({ name: block.name, args: block.args });
          this.log(`[tool_use] ${block.name}`);
        }
      }
      return;
    }
    if (method === 'tool_call_start') {
      this.log(`[tool_start] ${msg.params?.name}`);
      return;
    }
    if (method === 'tool_call_done') {
      this.log(`[tool_done] ${msg.params?.name} success=${msg.params?.success}`);
      return;
    }
    if (method === 'session.done') {
      this.done = true;
      this.doneReason = msg.params?.reason;
      this.log(`[session.done] reason=${this.doneReason}`);
      return;
    }
    if (method === 'session.plan_created') {
      this.log(`[plan_created]`);
      return;
    }
  }

  async sendMessage(content) {
    this.responseText = '';
    this.toolCalls = [];
    this.done = false;
    this.doneReason = '';
    send(this.ws, 'sessions.send', { session_id: this.sessionId, content });
    // 等待 session.done
    await this.waitForDone(90000);
  }

  waitForDone(timeout = 60000) {
    return new Promise((resolve) => {
      const start = Date.now();
      const check = () => {
        if (this.done) { resolve(); return; }
        if (Date.now() - start > timeout) { resolve(); return; }
        setTimeout(check, 200);
      };
      check();
    });
  }

  async run() {
    await this.connect();

    // ===== 测试1: 基础对话 =====
    console.log('\n--- Test 1: 基础对话 ---');
    await this.sendMessage('Hello! Please reply with exactly: "E2E_TEST_OK" and nothing else.');
    if (this.responseText.length > 0) {
      this.pass('基础对话-收到回复');
      if (this.doneReason === 'completed') {
        this.pass('基础对话-session.done(completed)');
      } else {
        this.fail('基础对话-session.done', `reason=${this.doneReason}`);
      }
    } else {
      this.fail('基础对话-收到回复', 'responseText 为空');
    }

    // ===== 测试2: 工具调用 (list_files) =====
    console.log('\n--- Test 2: 工具调用 list_files ---');
    await this.sendMessage('List the files in the current directory using the list_files tool. Do NOT read file contents, just list.');
    const toolNames = this.toolCalls.map(tc => tc.name);
    if (toolNames.length > 0) {
      this.pass('工具调用-模型调用了工具');
      if (toolNames.includes('list_files') || toolNames.includes('ls') || toolNames.includes('read') || toolNames.includes('bash')) {
        this.pass('工具调用-调用了文件相关工具');
      } else {
        this.fail('工具调用-文件相关工具', `调用了: ${toolNames.join(',')}`);
      }
      if (this.doneReason === 'completed') {
        this.pass('工具调用-session.done(completed)');
      } else {
        this.fail('工具调用-session.done', `reason=${this.doneReason}`);
      }
    } else {
      this.fail('工具调用-模型调用了工具', '无工具调用');
    }

    // ===== 测试3: 工具调用 (read) =====
    console.log('\n--- Test 3: 工具调用 read ---');
    await this.sendMessage('Read the file README.md in the current directory using the read tool, then tell me the first line.');
    const readTools = this.toolCalls.map(tc => tc.name);
    if (readTools.includes('read')) {
      this.pass('read工具-调用了read');
    } else {
      this.fail('read工具-调用了read', `调用了: ${readTools.join(',')}`);
    }
    if (this.responseText.length > 0) {
      this.pass('read工具-有文本回复');
    } else {
      this.fail('read工具-有文本回复', '无文本');
    }

    // ===== 测试4: bash 工具 =====
    console.log('\n--- Test 4: 工具调用 bash ---');
    await this.sendMessage('Run "echo E2E_BASH_OK" using the bash tool and tell me the output.');
    const bashTools = this.toolCalls.map(tc => tc.name);
    if (bashTools.includes('bash')) {
      this.pass('bash工具-调用了bash');
    } else {
      this.fail('bash工具-调用了bash', `调用了: ${bashTools.join(',')}`);
    }
    if (this.doneReason === 'completed') {
      this.pass('bash工具-session.done(completed)');
    } else {
      this.fail('bash工具-session.done', `reason=${this.doneReason}`);
    }

    // ===== 测试5: plan 工作流 =====
    console.log('\n--- Test 5: plan 工作流 ---');
    await this.sendMessage('Create a plan with 3 steps to write a hello world script. Use the plan tool.');
    const planTools = this.toolCalls.map(tc => tc.name);
    if (planTools.includes('plan') || planTools.includes('plan_create')) {
      this.pass('plan工作流-调用了plan工具');
    } else {
      this.fail('plan工作流-调用了plan工具', `调用了: ${planTools.join(',')}`);
    }

    // ===== 测试6: todo 工作流 =====
    console.log('\n--- Test 6: todo 工作流 ---');
    await this.sendMessage('Create 2 todos: "task1" and "task2" using the todo tool.');
    const todoTools = this.toolCalls.map(tc => tc.name);
    if (todoTools.includes('todo')) {
      this.pass('todo工作流-调用了todo工具');
    } else {
      this.fail('todo工作流-调用了todo工具', `调用了: ${todoTools.join(',')}`);
    }

    // ===== 总结 =====
    console.log('\n========== E2E TEST SUMMARY ==========');
    console.log(`PASS: ${this.results.pass}  FAIL: ${this.results.fail}`);
    for (const t of this.results.tests) {
      console.log(`  ${t.status === 'pass' ? '✓' : '✗'} ${t.name}${t.reason ? ' (' + t.reason + ')' : ''}`);
    }
    console.log('======================================');

    this.ws.close();
    process.exit(this.results.fail > 0 ? 1 : 0);
  }
}

const timeout = setTimeout(() => {
  console.error('\n[GLOBAL TIMEOUT] 120s reached');
  process.exit(1);
}, 120000);

new TestRunner().run().catch(err => {
  console.error('Fatal:', err);
  process.exit(1);
});
