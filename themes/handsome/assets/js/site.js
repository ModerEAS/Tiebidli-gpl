/* 前台胶水（自写）：暗色开关、目录、朗读、阅读模式、字号、点赞、PJAX、播放器、日历/雷达占位。
   依赖均为公开 CDN（jQuery/pjax/highlight/aplayer），加载失败时逐个降级，页面仍可用。 */
(function () {
  'use strict';
  var TB = window.TB || {};

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }

  onReady(function () {
    // 暗色开关（默认 light，与 LocalConst THEME_DARK_MODE 一致）。
    var dark = null;
    try { dark = localStorage.getItem('tb-dark'); } catch (e) {}
    if (dark === '1') document.body.classList.add('dark');
    var toggle = document.getElementById('dark_mode_toggle');
    if (toggle) toggle.addEventListener('click', function (ev) {
      ev.preventDefault();
      var on = document.body.classList.toggle('dark');
      try { localStorage.setItem('tb-dark', on ? '1' : '0'); } catch (e) {}
    });

    // 移动端导航 + 通用 toggle。
    document.querySelectorAll('[data-toggle-class]').forEach(function (btn) {
      btn.addEventListener('click', function () {
        var sel = btn.getAttribute('data-target');
        if (!sel) return;
        document.querySelectorAll(sel).forEach(function (el) {
          el.classList.toggle(btn.getAttribute('data-toggle-class'));
        });
      });
    });

    // 搜索框（Ctrl+K 打开）。
    var modal = document.getElementById('search_modal');
    var input = document.getElementById('search_input');
    function openSearch(ev) {
      if (ev) ev.preventDefault();
      if (!modal) return;
      modal.classList.remove('hide');
      if (input) input.focus();
    }
    var trigger = document.getElementById('search_trigger');
    if (trigger) trigger.addEventListener('click', openSearch);
    document.addEventListener('keydown', function (ev) {
      if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'k') openSearch(ev);
      if (ev.key === 'Escape' && modal) modal.classList.add('hide');
    });

    // TOC：从正文 h2/h3 生成。
    var toc = document.getElementById('tocTree');
    var content = document.getElementById('post-content');
    if (toc && content) {
      var heads = content.querySelectorAll('h2, h3');
      if (heads.length) {
        var ul = document.createElement('ul');
        heads.forEach(function (h, i) {
          if (!h.id) h.id = 'toc-' + i;
          var li = document.createElement('li');
          var a = document.createElement('a');
          a.href = '#' + h.id;
          a.textContent = h.textContent;
          li.appendChild(a);
          ul.appendChild(li);
        });
        toc.appendChild(ul);
      }
    }

    // 代码高亮（highlight.js 存在才跑）。
    try {
      if (window.hljs) document.querySelectorAll('pre code').forEach(function (b) { window.hljs.highlightElement(b); });
    } catch (e) {}

    // 朗读。
    var speechBtn = document.getElementById('post_speech');
    if (speechBtn && 'speechSynthesis' in window) {
      speechBtn.addEventListener('click', function () {
        if (window.speechSynthesis.speaking) { window.speechSynthesis.cancel(); return; }
        var text = content ? content.innerText : document.body.innerText;
        window.speechSynthesis.speak(new window.SpeechSynthesisUtterance(text.slice(0, 8000)));
      });
    } else if (speechBtn) {
      speechBtn.style.display = 'none';
    }

    // 阅读模式 + 字号。
    var readBtn = document.getElementById('read_mode_toggle');
    if (readBtn) readBtn.addEventListener('click', function () { document.body.classList.toggle('read-mode'); });
    var size = 0;
    function font(d) {
      size = Math.max(-2, Math.min(4, size + d));
      if (content) content.style.fontSize = (100 + size * 8) + '%';
    }
    var dec = document.getElementById('font_size_dec');
    var inc = document.getElementById('font_size_inc');
    if (dec) dec.addEventListener('click', function () { font(-1); });
    if (inc) inc.addEventListener('click', function () { font(1); });

    // 点赞：POST /?action=star_post&cid=；回 1 成功、2 已赞、-1 失败。
    function star(url, ok) {
      fetch(url, { method: 'POST', credentials: 'same-origin' }).then(function (r) { return r.text(); }).then(function (t) {
        t = (t || '').trim();
        if (t === '1') ok();
        else if (t === '2') alert(TB.STAR_REPEAT || '已赞过');
        else alert(TB.STAR_ERROR_NETWORK || '请求失败');
      }).catch(function () { alert(TB.STAR_ERROR_NETWORK || '请求失败'); });
    }
    var like = document.getElementById('star_post');
    if (like) like.addEventListener('click', function () {
      var cid = like.getAttribute('data-cid');
      star('/?action=star_post&cid=' + encodeURIComponent(cid), function () {
        var label = document.getElementById('like_label');
        if (label) label.textContent = String(parseInt(label.textContent || '0', 10) + 1);
        alert(TB.STAR_SUCCESS || '成功');
      });
    });
    document.querySelectorAll('.star_talk').forEach(function (el) {
      el.addEventListener('click', function (ev) {
        ev.preventDefault();
        var coid = el.getAttribute('data-coid');
        star('/?action=star_talk&coid=' + encodeURIComponent(coid), function () {
          var n = el.querySelector('.star_count');
          if (n) n.textContent = String(parseInt(n.textContent || '0', 10) + 1);
        });
      });
    });

    // PJAX（jquery-pjax 存在才接管，容器 #content）。
    try {
      if (window.jQuery && window.jQuery.pjax) {
        window.jQuery(document).pjax('a[href^="/"]:not([target]):not([data-no-pjax])', '#content', { timeout: 8000 });
      }
    } catch (e) {}

    // 日历/雷达占位：有数据则列出入口，无 ECharts 依赖。
    function fill(id, items, fmt) {
      var el = document.getElementById(id);
      if (!el || !items || !items.length) return;
      el.classList.remove('hide');
      var ul = document.createElement('ul');
      items.slice(0, 24).forEach(function (it) {
        var li = document.createElement('li');
        var a = document.createElement('a');
        a.href = it.url; a.textContent = fmt(it);
        li.appendChild(a); ul.appendChild(li);
      });
      el.appendChild(ul);
    }
    fill('post-calendar', TB.CALENDAR, function (it) { return it.year + '-' + it.month; });
    fill('category-radar', TB.CATS, function (it) { return it.name + ' (' + it.count + ')'; });

    // 音乐：`window.TB_MUSIC = {server, type, id}` 配好后才初始化 APlayer。
    var music = window.TB_MUSIC;
    var player = document.getElementById('handsome_global_player');
    if (music && music.id && player && window.APlayer && TB.MUSIC_API) {
      var api = TB.MUSIC_API.split('?')[0] + '?server=' + encodeURIComponent(music.server || 'netease') +
        '&type=' + encodeURIComponent(music.type || 'playlist') + '&id=' + encodeURIComponent(music.id);
      fetch(api, { credentials: 'same-origin' }).then(function (r) { return r.json(); }).then(function (list) {
        if (!list || !list.length) return;
        player.classList.remove('hide');
        new window.APlayer({ container: player, audio: list });
      }).catch(function () {});
    }
  });
})();
