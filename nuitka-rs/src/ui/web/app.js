(function () {
  const bridge = window.__TAURI__ && window.__TAURI__.core;
  const events = window.__TAURI__ && window.__TAURI__.event;

  const $ = (id) => document.getElementById(id);
  const wizard = $('wizard');
  const main = $('main');
  const verdict = document.querySelector('.verdict');

  let mode = 'enter';

  function setBusy(text) {
    $('lead').textContent = text;
    $('ring').classList.remove('hidden');
    $('result').classList.add('hidden');
    $('primary').disabled = true;
    $('prepare').disabled = true;
  }

  function showResult(status) {
    $('ring').classList.add('hidden');
    $('result').classList.remove('hidden');
    $('primary').disabled = false;
    $('prepare').disabled = false;

    verdict.className = 'verdict';
    if (status.level === '优秀' || status.level === '可用') verdict.classList.add('ok');
    else if (status.level === '待完善') verdict.classList.add('warn');
    else verdict.classList.add('bad');

    $('levelText').textContent = '环境状态：' + status.level;
    $('pythonText').textContent = status.python;
    $('pythonPath').textContent = status.python_path || '';
    $('nuitkaText').textContent = status.nuitka;
    $('compilerText').textContent = status.compiler;
    $('hint').textContent = status.hint;

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

  async function loadReport() {
    try {
      const data = await bridge.invoke('scan');
      $('rPython').textContent = data.python || '—';
      $('rNuitka').textContent = data.nuitka || '—';
      $('rCompilers').textContent = data.compilers || '—';
      $('rRec').textContent = data.recommendation || '—';
    } catch (error) {
      $('rCompilers').textContent = '扫描失败，请稍后重试。';
    }
  }

  $('primary').addEventListener('click', async function () {
    if (mode === 'enter') {
      await bridge.invoke('enter_main');
      wizard.classList.add('hidden');
      main.classList.remove('hidden');
      loadReport();
      return;
    }
    if (mode === 'retry') {
      refreshStatus();
      return;
    }
    await prepareEnv();
  });

  $('prepare').addEventListener('click', prepareEnv);
  $('rescan').addEventListener('click', loadReport);

  window.addEventListener('DOMContentLoaded', refreshStatus);
})();
