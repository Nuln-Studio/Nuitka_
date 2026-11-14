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
    compiler: '',
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
  let building = false;
  let previewTimer = null;
  let ticker = null;
  let startedAt = 0;

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

  function setBuildHead(kind) {
    const title = $('buildTitle');
    const badge = $('buildBadge');
    if (title) {
      title.textContent = kind === 'done' ? '打包完成' : kind === 'failed' ? '打包失败' : '正在打包';
    }
    if (badge) {
      badge.className = 'build-badge ' + kind;
      badge.textContent = kind === 'done' ? 'DONE' : kind === 'failed' ? 'FAILED' : 'RUNNING';
    }
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
      state.compiler = status.compiler_kind || '';
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

  /* ---------------- 页面切换 ---------------- */

  function showPage(name) {
    $('pageConfig').classList.toggle('hidden', name !== 'config');
    $('pageBuild').classList.toggle('hidden', name !== 'build');
  }

  let gaugeTarget = 0;
  let gaugeShown = 0;
  let gaugeRaf = null;
  let gaugeCreep = null;
  let gaugeRunning = false;

  function renderGauge(value) {
    const clamped = Math.max(0, Math.min(100, value));
    $('gaugeBar').style.width = clamped + '%';
    $('gaugePercent').textContent = String(Math.round(clamped));
  }

  // 逐帧追目标值：目标是单调的，画面就是平滑的
  function tickGauge() {
    const diff = gaugeTarget - gaugeShown;
    if (Math.abs(diff) < 0.1) {
      gaugeShown = gaugeTarget;
    } else {
      gaugeShown += diff * 0.09 + (diff > 0 ? 0.05 : -0.05);
    }
    renderGauge(gaugeShown);
    if (gaugeRunning || Math.abs(gaugeTarget - gaugeShown) > 0.05) {
      gaugeRaf = requestAnimationFrame(tickGauge);
    } else {
      gaugeRaf = null;
    }
  }

  function startGaugeLoop() {
    if (!gaugeRaf) gaugeRaf = requestAnimationFrame(tickGauge);
  }

  function raiseGauge(percent) {
    if (typeof percent !== 'number' || !isFinite(percent)) return;
    // 只升不降：Nuitka 会把模块计数重来一遍，倒退的数值直接丢掉
    gaugeTarget = Math.max(gaugeTarget, percent);
    startGaugeLoop();
  }

  function resetGauge() {
    gaugeRunning = false;
    gaugeTarget = 0;
    gaugeShown = 0;
    renderGauge(0);
  }

  // 卡在某个阶段时慢慢往前挪，别让进度条像死了一样停住
  function beginGaugeCreep() {
    stopGaugeCreep();
    gaugeCreep = setInterval(function () {
      if (!gaugeRunning || gaugeTarget >= 96) return;
      gaugeTarget = Math.min(96, gaugeTarget + 0.4);
      startGaugeLoop();
    }, 700);
  }

  function stopGaugeCreep() {
    if (gaugeCreep) {
      clearInterval(gaugeCreep);
      gaugeCreep = null;
    }
  }

  function setGaugeState(kind) {
    const gauge = document.querySelector('.gauge');
    gauge.className = 'gauge' + (kind ? ' ' + kind : '');
  }

  function setStage(text) {
    if (typeof text === 'string' && text) $('buildStage').textContent = text;
  }

  function setMeta(text) {
    $('buildMeta').textContent = text || '';
  }

  /* ---------------- 弹窗 ---------------- */

  function showModal(title, body, kind) {
    const modal = document.querySelector('.modal');
    modal.classList.toggle('warn', kind === 'warn');
    $('modalTitle').textContent = title;
    $('modalBody').textContent = body;

    const showLog = kind !== 'warn';
    $('modalLog').classList.toggle('hidden', !showLog);
    document.querySelector('.modal-note').classList.toggle('hidden', !showLog);

    $('modalLayer').classList.remove('hidden');
  }

  function hideModal() {
    $('modalLayer').classList.add('hidden');
  }

  /* ---------------- 主界面：表单 ---------------- */

  function request() {
    return {
      python: state.pythonPath,
      script: state.script,
      out_dir: state.outDir,
      name: state.name,
      icon: state.icon,
      onefile: state.mode === 'onefile',
      console: state.console === 'force',
      show_progress: state.progress,
      download: state.download,
      compiler: state.compiler,
      extra: state.extra
    };
  }

  function canBuild() {
    return !!(state.script && state.pythonPath && state.ready);
  }

  function targetPath() {
    if (!state.script || !state.outDir) return '';
    const stem = (state.script.split(/[\\/]/).pop() || '').replace(/\.py$/i, '');
    const name = state.name || stem;
    const leaf = state.mode === 'onefile' ? name + '.exe' : name + '.dist\\';
    return state.outDir.replace(/[\\/]+$/, '') + '\\' + leaf;
  }

  function update() {
    $('scriptName').textContent = state.script ? baseName(state.script) : '未选择';
    $('scriptPath').textContent = state.script || '';
    $('scriptPath').title = state.script || '';
    $('outPath').textContent = state.outDir || '未选择';
    $('outPath').title = state.outDir || '';
    $('iconPath').textContent = state.icon ? baseName(state.icon) : '未选择';
    $('iconPath').title = state.icon || '';

    if (!building) {
      const status = $('statusText');
      if (!state.script) status.textContent = '尚未选择入口脚本';
      else if (!state.ready) status.textContent = '环境不完整，请返回向导准备环境';
      else if (!state.outDir) status.textContent = '还没有选择输出目录';
      else status.textContent = '就绪，可以开始打包';
    }

    $('startBuild').disabled = building || !canBuild();
    $('resetForm').disabled = building;
    $('copyCmd').disabled = building;

    const hint = $('targetHint');
    hint.textContent = targetPath();
  }

  function bindSegmented(id, key) {
    const group = $(id);
    group.addEventListener('click', function (event) {
      const button = event.target.closest('button');
      if (!button || building) return;
      state[key] = button.dataset.value;
      Array.prototype.forEach.call(group.children, function (child) {
        child.classList.toggle('on', child === button);
      });
      update();
    });
  }

  /* ---------------- 打包 ---------------- */

  function startTicker() {
    stopTicker();
    startedAt = Date.now();
    ticker = setInterval(function () {
      setMeta('已用时 ' + Math.round((Date.now() - startedAt) / 1000) + ' 秒');
    }, 1000);
    setMeta('已用时 0 秒');
  }

  function stopTicker() {
    if (ticker) {
      clearInterval(ticker);
      ticker = null;
    }
  }

  function finishBuild() {
    building = false;
    gaugeRunning = false;
    stopGaugeCreep();
    stopTicker();
    $('cancelBuild').classList.add('hidden');
    $('startBuild').disabled = !canBuild();
    $('resetForm').disabled = false;
    $('copyCmd').disabled = false;
  }

  async function startBuild() {
    if (building) return;
    if (!state.script) {
      showModal('还差一步', '先选一个要打包的 Python 脚本。', 'warn');
      return;
    }
    if (!state.ready) {
      showModal('环境还没准备好', '请回到启动向导，点「准备环境」把 Python 和编译器装好。', 'warn');
      return;
    }
    if (!state.outDir) {
      showModal('还没选输出目录', '打包生成的 exe 需要一个地方存放。', 'warn');
      return;
    }

    building = true;
    setBuildHead('running');
    $('buildScript').textContent = baseName(state.script);
    $('buildScript').title = state.script;
    $('buildOut').textContent = state.outDir;
    $('buildOut').title = state.outDir;
    $('buildMode').textContent = state.mode === 'onefile' ? '单文件' : '单目录';
    $('buildTargetPath').textContent = targetPath();
    $('buildTargetPath').title = targetPath();
    showPage('build');
    setGaugeState('running');
    resetGauge();
    gaugeRunning = true;
    beginGaugeCreep();
    setStage('正在收集环境信息…');
    startTicker();
    $('cancelBuild').classList.remove('hidden');
    $('openOut').classList.add('hidden');
    $('buildAgain').classList.add('hidden');
    $('startBuild').disabled = true;

    try {
      const result = await bridge.invoke('start_build', { request: request() });
      if (result.ok) {
        gaugeRunning = false;
        gaugeTarget = 100;
        gaugeShown = 100;
        renderGauge(100);
        setGaugeState('done');
        setBuildHead('done');
        setStage('打包完成');
        setMeta('用时 ' + result.seconds + ' 秒');
        $('openOut').classList.remove('hidden');
        $('buildAgain').classList.remove('hidden');
      } else {
        gaugeRunning = false;
        setGaugeState('failed');
        setBuildHead('failed');
        setStage('打包失败');
        setMeta('');
        showModal('打包失败', result.detail);
      }
    } catch (error) {
      gaugeRunning = false;
      setGaugeState('failed');
      setBuildHead('failed');
      setStage('打包失败');
      setMeta('');
      showModal('打包失败', String(error));
    } finally {
      finishBuild();
    }
  }

  /* ---------------- 主界面：交互 ---------------- */

  $('pickScript').addEventListener('click', async function () {
    if (building) return;
    const picked = await bridge.invoke('pick_script', { title: '选择要打包的 Python 脚本' });
    if (!picked) return;
    state.script = picked;
    update();
  });

  $('pickOut').addEventListener('click', async function () {
    if (building) return;
    const picked = await bridge.invoke('pick_dir', { title: '选择输出目录' });
    if (!picked) return;
    state.outDir = picked;
    update();
  });

  $('pickIcon').addEventListener('click', async function () {
    if (building) return;
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
  });

  $('optDownload').addEventListener('change', function (event) {
    state.download = event.target.checked;
  });

  $('resetForm').addEventListener('click', function () {
    if (building) return;
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
    if (!state.script) return;
    let text = '';
    try {
      text = await bridge.invoke('preview_command', { request: request() });
    } catch (error) {
      return;
    }
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
    setTimeout(function () { button.textContent = '复制命令'; }, 1400);
  });

  $('startBuild').addEventListener('click', startBuild);

  $('cancelBuild').addEventListener('click', async function () {
    setStage('正在取消…');
    try {
      await bridge.invoke('cancel_build');
    } catch (error) {
      setStage('取消失败');
    }
  });

  $('openOut').addEventListener('click', function () {
    if (state.outDir) bridge.invoke('open_path', { path: state.outDir });
  });

  $('buildAgain').addEventListener('click', function () {
    showPage('config');
  });

  $('modalClose').addEventListener('click', function () {
    hideModal();
    if (!building) showPage('config');
  });

  $('modalLog').addEventListener('click', async function () {
    try {
      await bridge.invoke('open_log');
    } catch (error) {
      $('modalBody').textContent = '打不开日志文件，详细信息见程序目录下的 nuitka-rs.log';
    }
  });

  $('modalLayer').addEventListener('click', function (event) {
    if (event.target === $('modalLayer')) {
      hideModal();
      if (!building) showPage('config');
    }
  });

  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Escape') return;
    if ($('modalLayer').classList.contains('hidden')) return;
    hideModal();
    if (!building) showPage('config');
  });

  $('rescan').addEventListener('click', function () {
    reportLoaded = false;
    loadEnv();
  });

  $('envToggle').addEventListener('click', loadReport);

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
      ? '<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><rect x="2" y="3.4" width="4.6" height="4.6" rx="1" fill="none" stroke="currentColor" stroke-width="1.1"/><path d="M3.6 2.1h4.3v4.3" fill="none" stroke="currentColor" stroke-width="1.1"/></svg>'
      : '<svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true"><rect x="2.3" y="2.3" width="5.4" height="5.4" rx="1" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>';
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
      showPage('config');
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

  if (events) {
    events.listen('build-progress', function (event) {
      if (!building) return;
      const payload = event.payload || {};
      raiseGauge(payload.percent);
      setStage(payload.stage);
    });
  }

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
