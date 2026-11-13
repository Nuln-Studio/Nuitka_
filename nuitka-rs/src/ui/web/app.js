(function () {
  const bridge = window.__TAURI__ && window.__TAURI__.core;
  const events = window.__TAURI__ && window.__TAURI__.event;
  const appWindow = window.__TAURI__ && window.__TAURI__.window
    ? window.__TAURI__.window.getCurrentWindow()
    : null;

  const $ = (id) => document.getElementById(id);
  const wizard = $('wizard');
  const main = $('main');
  const verdict = document.querySelector('.verdict');

  const state = {
    python: '',
    pythonPath: '',
    ready: false,
    script: '',
    outDir: '',
    name: '',
    icon: '',
    mode: 'onefile',
    console: 'force',
    progress: true,
    download: true,
    extra: ''
  };

  let mode = 'enter';
  let reportLoaded = false;

  /* ---------------- 通用 ---------------- */

  function setBusy(text) {
    $('lead').textContent = text;
    $('ring').classList.remove('hidden');
    $('result').classList.add('hidden');
    $('primary').disabled = true;
    $('prepare').disabled = true;
  }

  function baseName(path) {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] || path;
  }

  function quote(value) {
    return '"' + String(value).replace(/"/g, '\\"') + '"';
  }

  /* ---------------- 启动向导 ---------------- */

  function showResult(status) {
    $('ring').classList.add('hidden');
    $('result').classList.remove('hidden');
    $('primary').disabled = false;
    $('prepare').disabled = false;

    verdict.className = 'verdict';
    if (status.level === '优秀' || status.level === '可用') verdict.classList.add('ok');
    else if (status.level === '待验证' || status.level === '待完善') verdict.classList.add('warn');
    else verdict.classList.add('bad');

    $('levelText').textContent = '环境状态：' + status.level;
    $('pythonText').textContent = status.python;
    $('pythonPath').textContent = status.python_path || '';
    $('nuitkaText').textContent = status.nuitka;
    $('compilerText').textContent = status.compiler;
    $('hint').textContent = status.hint;

    state.python = status.python || '';
    state.pythonPath = status.python_path || '';
    state.ready = !!status.ready;

    if (status.ready) {
      mode = 'enter';
      $('primary').textContent = '进入主界面';
      $('lead').textContent = '环境检查完成。';
    } else {
      mode = 'prepare';
      $('primary').textContent = '立即准备环境';
      $('lead').textContent = '环境尚不完整。';
    }
  }

  async function refreshStatus() {
    if (!bridge) {
      $('lead').textContent = '未检测到应用环境，请从桌面程序启动。';
      $('ring').classList.add('hidden');
      return;
    }
    setBusy('正在检查编译环境…');
    try {
      const status = await bridge.invoke('env_status');
      showResult(status);
    } catch (error) {
      $('ring').classList.add('hidden');
      $('lead').textContent = '环境检查失败，请稍后重试。';
      $('primary').disabled = false;
      $('primary').textContent = '重新检查';
      mode = 'retry';
    }
  }

  async function prepareEnv() {
    let base = null;
    try {
      base = await bridge.invoke('pick_dir', { title: '选择环境安装目录' });
    } catch (error) {
      base = null;
    }
    if (!base) return;

    setBusy('正在准备环境，请勿关闭窗口…');
    $('prepare').disabled = true;

    if (events) {
      events.listen('env-progress', function (event) {
        $('lead').textContent = event.payload;
      });
    }

    try {
      const status = await bridge.invoke('prepare_env', { base: base });
      showResult(status);
      $('lead').textContent = '环境准备完成。';
    } catch (error) {
      $('ring').classList.add('hidden');
      $('lead').textContent = '环境准备失败，详细信息见程序目录下的 nuitka-rs.log。';
      $('primary').disabled = false;
      $('prepare').disabled = false;
      mode = 'prepare';
    }
  }

  /* ---------------- 主界面：环境条 ---------------- */

  function levelClass(level) {
    if (level === '优秀' || level === '可用') return 'ok';
    if (level === '待验证' || level === '待完善') return 'warn';
    return 'bad';
  }

  async function loadEnv() {
    $('envDot').className = 'env-dot';
    $('envText').textContent = '正在检测环境…';
    try {
      const status = await bridge.invoke('env_status');
      state.python = status.python || '';
      state.pythonPath = status.python_path || '';
      state.ready = !!status.ready;
      $('envDot').className = 'env-dot ' + levelClass(status.level);
      let text = [status.python, status.nuitka, status.compiler].join(' · ');
      if (status.ready && !status.verified) text += ' · 未跑编译自检';
      $('envText').textContent = text;
    } catch (error) {
      $('envDot').className = 'env-dot bad';
      $('envText').textContent = '环境检测失败，详细信息见 nuitka-rs.log';
      state.ready = false;
    }
    update();
  }

  async function loadReport() {
    const box = $('envReport');
    if (!box.classList.contains('hidden')) {
      box.classList.add('hidden');
      return;
    }
    box.classList.remove('hidden');
    if (reportLoaded) return;
    try {
      const data = await bridge.invoke('scan');
      box.textContent = [
        'Python  ' + (data.python || '—'),
        'Nuitka  ' + (data.nuitka || '—'),
        '编译器  ' + (data.compilers || '—'),
        '编译自检 ' + (data.selfcheck || '—'),
        '推荐    ' + (data.recommendation || '—')
      ].join('\n\n');
      reportLoaded = true;
    } catch (error) {
      box.textContent = '扫描失败，详细信息见 nuitka-rs.log';
    }
  }

  /* ---------------- 主界面：表单 ---------------- */

  function commandText() {
    if (!state.script) return '';
    const parts = [];
    parts.push(quote(state.pythonPath || 'python'));
    parts.push('-m nuitka');
    parts.push(state.mode === 'onefile' ? '--onefile' : '--standalone');
    parts.push('--windows-console-mode=' + state.console);
    if (state.progress) parts.push('--show-progress');
    if (state.download) parts.push('--assume-yes-for-downloads');
    if (state.outDir) parts.push('--output-dir=' + quote(state.outDir));
    if (state.name) parts.push('--output-filename=' + quote(state.name));
    if (state.icon) parts.push('--windows-icon-from-ico=' + quote(state.icon));
    const extra = state.extra.trim();
    if (extra) parts.push(extra);
    parts.push(quote(state.script));
    return parts.join(' ');
  }

  function update() {
    $('scriptName').textContent = state.script ? baseName(state.script) : '未选择';
    $('scriptPath').textContent = state.script || '';
    $('outPath').textContent = state.outDir || '未选择';
    $('iconPath').textContent = state.icon ? baseName(state.icon) : '未选择';
    $('iconPath').title = state.icon || '';

    const command = commandText();
    $('cmdText').textContent = command || '选择入口脚本后，这里会生成等价的 Nuitka 命令行。';

    const status = $('statusText');
    if (!state.script) {
      status.textContent = '尚未选择入口脚本';
    } else if (!state.ready) {
      status.textContent = '环境不完整，请返回向导准备环境';
    } else if (!state.outDir) {
      status.textContent = '还没有选择输出目录';
    } else {
      status.textContent = '命令已生成，可复制后手动执行；一键打包将随 M2 接通';
    }

    const canBuild = state.script && state.ready && state.outDir;
    $('startBuild').disabled = true;
    $('startBuild').title = canBuild
      ? '打包后端尚未接通（M2）'
      : '请先选择脚本、输出目录，并确认环境完整';
    $('startBuild').dataset.ready = canBuild ? '1' : '';
  }

  function bindSegmented(id, key) {
    const group = $(id);
    group.addEventListener('click', function (event) {
      const button = event.target.closest('button');
      if (!button) return;
      state[key] = button.dataset.value;
      Array.prototype.forEach.call(group.children, function (child) {
        child.classList.toggle('on', child === button);
      });
      update();
    });
  }

  /* ---------------- 主界面：交互 ---------------- */

  $('pickScript').addEventListener('click', async function () {
    const picked = await bridge.invoke('pick_script', { title: '选择要打包的 Python 脚本' });
    if (!picked) return;
    state.script = picked;
    update();
  });

  $('pickOut').addEventListener('click', async function () {
    const picked = await bridge.invoke('pick_dir', { title: '选择输出目录' });
    if (!picked) return;
    state.outDir = picked;
    update();
  });

  $('pickIcon').addEventListener('click', async function () {
    const picked = await bridge.invoke('pick_icon', { title: '选择程序图标' });
    if (!picked) return;
    state.icon = picked;
    update();
  });

  $('appName').addEventListener('input', function (event) {
    state.name = event.target.value.trim();
    update();
  });

  $('extraArgs').addEventListener('input', function (event) {
    state.extra = event.target.value;
    update();
  });

  $('optProgress').addEventListener('change', function (event) {
    state.progress = event.target.checked;
    update();
  });

  $('optDownload').addEventListener('change', function (event) {
    state.download = event.target.checked;
    update();
  });

  $('resetForm').addEventListener('click', function () {
    state.script = '';
    state.outDir = '';
    state.name = '';
    state.icon = '';
    state.extra = '';
    state.mode = 'onefile';
    state.console = 'force';
    state.progress = true;
    state.download = true;
    $('appName').value = '';
    $('extraArgs').value = '';
    $('optProgress').checked = true;
    $('optDownload').checked = true;
    Array.prototype.forEach.call($('modeSeg').children, function (child, index) {
      child.classList.toggle('on', index === 0);
    });
    Array.prototype.forEach.call($('consoleSeg').children, function (child, index) {
      child.classList.toggle('on', index === 0);
    });
    update();
  });

  $('copyCmd').addEventListener('click', async function () {
    const text = commandText();
    if (!text) return;
    let copied = false;
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch (error) {
      const area = document.createElement('textarea');
      area.value = text;
      area.style.position = 'fixed';
      area.style.opacity = '0';
      document.body.appendChild(area);
      area.select();
      copied = document.execCommand('copy');
      document.body.removeChild(area);
    }
    const button = $('copyCmd');
    button.textContent = copied ? '已复制' : '复制失败';
    setTimeout(function () { button.textContent = '复制'; }, 1400);
  });

  $('rescan').addEventListener('click', function () {
    reportLoaded = false;
    loadEnv();
  });

  $('envToggle').addEventListener('click', loadReport);

  $('startBuild').addEventListener('click', function () {
    $('statusText').textContent = '打包后端尚未接通，M2 完成后这里会直接开始编译';
  });

  $('exportDiag').addEventListener('click', async function () {
    const button = $('exportDiag');
    button.disabled = true;
    try {
      const path = await bridge.invoke('export_diagnostics');
      $('statusText').textContent = '诊断快照已保存：' + path;
    } catch (error) {
      $('statusText').textContent = '导出诊断失败，详细信息见 nuitka-rs.log';
    }
    button.disabled = false;
  });

  /* ---------------- 窗口按钮 ---------------- */

  function paintMaximize(maximized) {
    const button = $('tb-max');
    if (!button) return;
    button.title = maximized ? '还原' : '最大化';
    button.innerHTML = maximized
      ? '<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><rect x="2" y="3.4" width="4.6" height="4.6" rx="0.9" fill="none" stroke="currentColor" stroke-width="1.05"/><path d="M3.6 2.1h4.3v4.3" fill="none" stroke="currentColor" stroke-width="1.05"/></svg>'
      : '<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><rect x="2.3" y="2.3" width="5.4" height="5.4" rx="1" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>';
  }

  async function toggleMaximize() {
    if (!appWindow) return;
    try {
      const maximized = await appWindow.isMaximized();
      if (maximized) await appWindow.unmaximize();
      else await appWindow.maximize();
      paintMaximize(!maximized);
    } catch (error) {
      // 忽略：窗口状态拿不到时不影响其它操作
    }
  }

  /* ---------------- 启动 ---------------- */

  bindSegmented('modeSeg', 'mode');
  bindSegmented('consoleSeg', 'console');

  $('primary').addEventListener('click', async function () {
    if (mode === 'enter') {
      await bridge.invoke('enter_main');
      document.body.classList.remove('wizard-mode');
      wizard.classList.add('hidden');
      main.classList.remove('hidden');
      update();
      loadEnv();
      return;
    }
    if (mode === 'retry') {
      refreshStatus();
      return;
    }
    await prepareEnv();
  });

  $('prepare').addEventListener('click', prepareEnv);

  if (appWindow) {
    $('tb-min').addEventListener('click', function () { appWindow.minimize(); });
    $('tb-max').addEventListener('click', toggleMaximize);
    $('tb-close').addEventListener('click', function () { appWindow.close(); });

    $('titlebar').addEventListener('dblclick', function (event) {
      if (event.target.closest('.tb-btn')) return;
      toggleMaximize();
    });

    appWindow.onResized(async function () {
      try {
        paintMaximize(await appWindow.isMaximized());
      } catch (error) {
        // 忽略
      }
    });
  }

  window.addEventListener('DOMContentLoaded', refreshStatus);
})();
