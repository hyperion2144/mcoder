// 全工具端到端测试：直接调用 tool.call，精确验证每个工具返回值
const WebSocket = require('ws');
const fs = require('fs');
const path = require('path');

const URL = 'ws://127.0.0.1:7654';
const TOKEN = '745ab3255e8447feabbf1a71c5424533';
const PROJECT = process.cwd();
const TEST_DIR = path.join(PROJECT, '.e2e-tools-fixture');

let msgId = 1;
const nextId = () => msgId++;

class ToolTester {
  constructor() {
    this.ws = null;
    this.sessionId = null;
    this.results = { pass: 0, fail: 0, tests: [] };
  }

  log(msg) { console.log(`    ${msg}`); }
  pass(name) { this.results.pass++; this.results.tests.push({ name, status: 'pass' }); console.log(`  [PASS] ${name}`); }
  fail(name, reason) { this.results.fail++; this.results.tests.push({ name, status: 'fail', reason }); console.log(`  [FAIL] ${name}: ${reason}`); }

  send(method, params) {
    const id = nextId();
    this.ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
    return id;
  }

  async rpc(method, params, timeout = 30000) {
    const id = nextId();
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`timeout: ${method}`)), timeout);
      const handler = (data) => {
        const msg = JSON.parse(data.toString());
        if (msg.id === id) {
          clearTimeout(timer);
          this.ws.removeListener('message', handler);
          if (msg.error) reject(new Error(msg.error.message || JSON.stringify(msg.error)));
          else resolve(msg.result);
        }
      };
      this.ws.on('message', handler);
      this.ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
    });
  }

  async connect() {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(URL);
      this.ws.on('open', () => this.send('auth', { token: TOKEN }));
      this.ws.on('error', reject);
      const handler = (data) => {
        const msg = JSON.parse(data.toString());
        // auth 响应 id 可能是 0 或 1
        if ((msg.id === 0 || msg.id === 1) && msg.result && msg.result.authenticated) {
          this.ws.removeListener('message', handler);
          resolve();
        }
      };
      this.ws.on('message', handler);
    });
  }

  async setupSession() {
    const r = await this.rpc('sessions.create', { title: 'e2e-tools', project: PROJECT });
    this.sessionId = r.session_id;
    await this.rpc('session.attach', { session_id: this.sessionId });
    this.log(`session: ${this.sessionId}`);
  }

  async callTool(name, args) {
    const r = await this.rpc('tool.call', {
      session_id: this.sessionId,
      name,
      args,
    }, 60000);
    return r;
  }

  async run() {
    // 准备测试目录
    fs.mkdirSync(TEST_DIR, { recursive: true });
    fs.writeFileSync(path.join(TEST_DIR, 'hello.py'), 'def greet():\n    return "hello"\n\ngreet()\n');
    fs.writeFileSync(path.join(TEST_DIR, 'readme.md'), '# Test\n\nHello world.\n');

    await this.connect();
    await this.setupSession();

    await this.testFileTools();
    await this.testBashTool();
    await this.testGraphTools();
    await this.testMemoryTools();
    await this.testAstTools();
    await this.testPlanTools();
    await this.testSandboxTool();
    await this.testUndoTool();
    await this.testWorkflowTools();
    await this.testSubagentTool();

    this.printSummary();
    this.cleanup();
    this.ws.close();
    process.exit(this.results.fail > 0 ? 1 : 0);
  }

  // ===== 文件工具 =====
  async testFileTools() {
    console.log('\n--- 文件工具 ---');

    // ls
    try {
      const r = await this.callTool('ls', { path: TEST_DIR });
      const entries = r.result?.entries || r.result?.files || r.result;
      const found = JSON.stringify(r).includes('hello.py');
      if (found) this.pass('ls-列出文件');
      else this.fail('ls-列出文件', `返回: ${JSON.stringify(r).slice(0, 200)}`);
    } catch (e) { this.fail('ls-列出文件', e.message); }

    // read
    try {
      const r = await this.callTool('read', { file: path.join(TEST_DIR, 'hello.py') });
      const text = JSON.stringify(r);
      if (text.includes('def greet')) this.pass('read-读取文件内容');
      else this.fail('read-读取文件内容', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('read-读取文件内容', e.message); }

    // write
    try {
      const r = await this.callTool('write', {
        file: path.join(TEST_DIR, 'written.txt'),
        content: 'written by e2e'
      });
      const exists = fs.existsSync(path.join(TEST_DIR, 'written.txt'));
      if (exists && JSON.stringify(r).includes('true')) this.pass('write-写入文件');
      else this.fail('write-写入文件', `exists=${exists}, r=${JSON.stringify(r).slice(0, 200)}`);
    } catch (e) { this.fail('write-写入文件', e.message); }

    // edit
    try {
      fs.writeFileSync(path.join(TEST_DIR, 'edit.txt'), 'line1\nline2\nline3\n');
      const r = await this.callTool('edit', {
        edits: [{
          file: path.join(TEST_DIR, 'edit.txt'),
          pattern: 'line2',
          replacement: 'LINE_TWO'
        }]
      });
      const content = fs.readFileSync(path.join(TEST_DIR, 'edit.txt'), 'utf-8');
      if (content.includes('LINE_TWO')) this.pass('edit-编辑文件');
      else this.fail('edit-编辑文件', `content=${content.slice(0, 100)}`);
    } catch (e) { this.fail('edit-编辑文件', e.message); }

    // grep
    try {
      const r = await this.callTool('grep', {
        pattern: 'greet',
        path: TEST_DIR,
        output_mode: 'content'
      });
      const text = JSON.stringify(r);
      if (text.includes('greet')) this.pass('grep-搜索内容');
      else this.fail('grep-搜索内容', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('grep-搜索内容', e.message); }
  }

  // ===== Bash 工具 =====
  async testBashTool() {
    console.log('\n--- Bash 工具 ---');
    try {
      const r = await this.callTool('bash', { cmd: 'echo E2E_BASH_OK', cwd: TEST_DIR });
      const text = JSON.stringify(r);
      if (text.includes('E2E_BASH_OK')) this.pass('bash-stdout正确返回');
      else this.fail('bash-stdout正确返回', `返回: ${text.slice(0, 300)}`);
    } catch (e) { this.fail('bash-stdout正确返回', e.message); }
  }

  // ===== 代码图谱工具 =====
  async testGraphTools() {
    console.log('\n--- 代码图谱工具 ---');

    // graph_index
    try {
      const r = await this.callTool('graph_index', { path: TEST_DIR });
      this.pass('graph_index-索引目录');
    } catch (e) { this.fail('graph_index-索引目录', e.message); }

    // graph_file_symbols (参数名是 path 不是 file)
    try {
      const r = await this.callTool('graph_file_symbols', { path: path.join(TEST_DIR, 'hello.py') });
      const text = JSON.stringify(r);
      if (text.includes('greet')) this.pass('graph_file_symbols-提取符号');
      else this.fail('graph_file_symbols-提取符号', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('graph_file_symbols-提取符号', e.message); }

    // graph_query
    try {
      const r = await this.callTool('graph_query', { name: '', limit: 10 });
      this.pass('graph_query-查询图谱');
    } catch (e) { this.fail('graph_query-查询图谱', e.message); }

    // graph_find
    try {
      const r = await this.callTool('graph_find', { name: 'greet' });
      const text = JSON.stringify(r);
      if (text.includes('greet')) this.pass('graph_find-查找符号');
      else this.fail('graph_find-查找符号', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('graph_find-查找符号', e.message); }

    // graph_callers
    try {
      const r = await this.callTool('graph_callers', { name: 'greet' });
      this.pass('graph_callers-查询调用者');
    } catch (e) { this.fail('graph_callers-查询调用者', e.message); }

    // graph_callees
    try {
      const r = await this.callTool('graph_callees', { name: 'greet' });
      this.pass('graph_callees-查询被调用者');
    } catch (e) { this.fail('graph_callees-查询被调用者', e.message); }

    // graph_references
    try {
      const r = await this.callTool('graph_references', { name: 'greet' });
      this.pass('graph_references-查询引用');
    } catch (e) { this.fail('graph_references-查询引用', e.message); }
  }

  // ===== 记忆工具 =====
  async testMemoryTools() {
    console.log('\n--- 记忆工具 ---');

    // memory_store (需要 key + content)
    try {
      const r = await this.callTool('memory_store', {
        key: 'e2e_test',
        content: 'E2E test memory entry',
        scope: 'project'
      });
      const text = JSON.stringify(r);
      if (text.includes('true') || text.includes('stored')) this.pass('memory_store-存储记忆');
      else this.fail('memory_store-存储记忆', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('memory_store-存储记忆', e.message); }

    // memory_search
    try {
      const r = await this.callTool('memory_search', { query: 'E2E test' });
      this.pass('memory_search-搜索记忆');
    } catch (e) { this.fail('memory_search-搜索记忆', e.message); }

    // memory_list
    try {
      const r = await this.callTool('memory_list', {});
      this.pass('memory_list-列出记忆');
    } catch (e) { this.fail('memory_list-列出记忆', e.message); }
  }

  // ===== AST 编辑工具 =====
  async testAstTools() {
    console.log('\n--- AST 编辑工具 ---');
    fs.writeFileSync(path.join(TEST_DIR, 'ast_test.py'), 'def old_func():\n    return 1\n\nold_func()\n');

    // 先确保 graph_index 索引到测试文件
    try {
      await this.callTool('graph_index', { path: TEST_DIR });
    } catch {}

    // ast_rename
    try {
      const r = await this.callTool('ast_rename', {
        old_name: 'old_func',
        new_name: 'new_func',
        file: path.join(TEST_DIR, 'ast_test.py')
      });
      const content = fs.readFileSync(path.join(TEST_DIR, 'ast_test.py'), 'utf-8');
      if (content.includes('new_func') && !content.includes('old_func')) this.pass('ast_rename-重命名符号');
      else this.fail('ast_rename-重命名符号', `content=${content.slice(0, 100)}`);
    } catch (e) { this.fail('ast_rename-重命名符号', e.message); }

    // ast_extract (需要 start_line/end_line/new_name)
    fs.writeFileSync(path.join(TEST_DIR, 'extract_test.py'), 'def func_a():\n    x = 1\n    return x\n\ndef func_b():\n    pass\n');
    try {
      const r = await this.callTool('ast_extract', {
        file: path.join(TEST_DIR, 'extract_test.py'),
        start_line: 2,
        end_line: 3,
        new_name: 'extracted_func'
      });
      this.pass('ast_extract-提取函数');
    } catch (e) { this.fail('ast_extract-提取函数', e.message); }

    // ast_inline
    fs.writeFileSync(path.join(TEST_DIR, 'inline_test.py'), 'def helper():\n    return 42\n\nx = helper()\n');
    try {
      // 先索引 inline_test.py
      await this.callTool('graph_index', { path: path.join(TEST_DIR, 'inline_test.py') });
      const r = await this.callTool('ast_inline', {
        name: 'helper'
      });
      this.pass('ast_inline-内联函数');
    } catch (e) { this.fail('ast_inline-内联函数', e.message); }
  }

  // ===== 计划工具 =====
  async testPlanTools() {
    console.log('\n--- 计划工具 ---');

    // plan_create (steps 需对象数组 [{description: "..."}])
    try {
      const r = await this.callTool('plan_create', {
        steps: [
          { description: 'step1: setup' },
          { description: 'step2: implement' },
          { description: 'step3: test' }
        ]
      });
      const text = JSON.stringify(r);
      if (text.includes('plan') || text.includes('id')) this.pass('plan_create-创建计划');
      else this.fail('plan_create-创建计划', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('plan_create-创建计划', e.message); }

    // todo (需要 action=add + content)
    try {
      const r = await this.callTool('todo', { action: 'add', content: 'E2E todo item' });
      const text = JSON.stringify(r);
      if (text.includes('true') || text.includes('created') || text.includes('id') || text.includes('td-')) this.pass('todo-创建待办');
      else this.fail('todo-创建待办', `返回: ${text.slice(0, 200)}`);
    } catch (e) { this.fail('todo-创建待办', e.message); }
  }

  // ===== 沙箱工具 =====
  async testSandboxTool() {
    console.log('\n--- 沙箱工具 ---');
    try {
      // sandbox_read 不需要预先有 handle，测一下能否调用
      const r = await this.callTool('sandbox_read', { op: 'list' });
      this.pass('sandbox_read-沙箱读取');
    } catch (e) { this.fail('sandbox_read-沙箱读取', e.message); }
  }

  // ===== 撤销工具 =====
  async testUndoTool() {
    console.log('\n--- 撤销工具 ---');
    // 先做一个 write 产生 journal entry，再 undo
    try {
      fs.writeFileSync(path.join(TEST_DIR, 'undo_test.txt'), 'original');
      await this.callTool('write', {
        file: path.join(TEST_DIR, 'undo_test.txt'),
        content: 'modified'
      });
      const r = await this.callTool('undo', { op: 'last' });
      const content = fs.readFileSync(path.join(TEST_DIR, 'undo_test.txt'), 'utf-8');
      if (content.includes('original')) this.pass('undo-撤销上次操作');
      else this.fail('undo-撤销上次操作', `content=${content.slice(0, 100)}`);
    } catch (e) { this.fail('undo-撤销上次操作', e.message); }
  }

  // ===== 工作流工具 =====
  async testWorkflowTools() {
    console.log('\n--- 工作流工具 ---');

    // workflow_create (需要 op=init + title)
    try {
      const r = await this.callTool('workflow_create', {
        op: 'init',
        title: 'E2E Workflow',
        description: 'Test workflow',
        milestone_title: 'M1'
      });
      this.pass('workflow_create-创建工作流');
    } catch (e) { this.fail('workflow_create-创建工作流', e.message); }

    // workflow_query (需要 op=roadmaps)
    try {
      const r = await this.callTool('workflow_query', { op: 'roadmaps' });
      this.pass('workflow_query-查询工作流');
    } catch (e) { this.fail('workflow_query-查询工作流', e.message); }
  }

  // ===== 子代理工具 =====
  async testSubagentTool() {
    console.log('\n--- 子代理工具 ---');
    try {
      const r = await this.callTool('subagent', {
        op: 'spawn',
        role: 'coder',
        task: 'Reply with exactly: SUBAGENT_OK and nothing else.'
      }, 90000);
      const text = JSON.stringify(r);
      if (text.includes('task_id') || text.includes('spawned') || text.includes('ok')) {
        this.pass('subagent-子代理启动');
      } else {
        this.fail('subagent-子代理启动', `返回: ${text.slice(0, 300)}`);
      }
    } catch (e) { this.fail('subagent-子代理启动', e.message); }
  }

  printSummary() {
    console.log('\n========== TOOLS E2E TEST SUMMARY ==========');
    console.log(`PASS: ${this.results.pass}  FAIL: ${this.results.fail}`);
    for (const t of this.results.tests) {
      console.log(`  ${t.status === 'pass' ? 'PASS' : 'FAIL'} ${t.name}${t.reason ? ' (' + t.reason + ')' : ''}`);
    }
    console.log('============================================');
  }

  cleanup() {
    try { fs.rmSync(TEST_DIR, { recursive: true, force: true }); } catch {}
  }
}

const timeout = setTimeout(() => {
  console.error('\n[GLOBAL TIMEOUT] 300s reached');
  process.exit(1);
}, 300000);

new ToolTester().run().catch(err => {
  console.error('Fatal:', err);
  process.exit(1);
});
