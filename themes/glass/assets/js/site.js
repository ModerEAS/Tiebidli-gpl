/* 前台胶水（自写）：暗色开关、目录、朗读、阅读模式、字号、点赞、PJAX、播放器、日历/雷达占位。
   依赖均为公开 CDN（jQuery/pjax/highlight），加载失败时逐个降级，页面仍可用。 */
(function () {
  'use strict';
  var TB = window.TB || {};

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }

  onReady(function () {
    // 暗色开关（默认 light，与 LocalConst THEME_DARK_MODE 一致）。
    // 深浅色开关（默认深色贴近现网观感；light 为干净浅色备选）。
    var toggle = document.getElementById('dark_mode_toggle');
    function paintToggle() {
      if (toggle) toggle.textContent = document.body.classList.contains('light') ? '暗色' : '浅色';
    }
    var light = null;
    try { light = localStorage.getItem('tb-light'); } catch (e) {}
    if (light === '1') document.body.classList.add('light');
    else document.body.classList.add('dark');
    paintToggle();
    if (toggle) toggle.addEventListener('click', function (ev) {
      ev.preventDefault();
      var lightNow = document.body.classList.toggle('light');
      document.body.classList.toggle('dark', !lightNow);
      try { localStorage.setItem('tb-light', lightNow ? '1' : '0'); } catch (e) {}
      paintToggle();
    });

    // 侧栏折叠组（对齐现网）：点标题切换展开/收起；feather 线框图标（CDN 失败静默跳过，文字仍在）。
    document.querySelectorAll('#aside [data-nav-toggle]').forEach(function (t) {
      t.addEventListener('click', function (ev) {
        ev.preventDefault();
        var li = t.closest('li');
        if (li) li.classList.toggle('active');
      });
    });
    try { if (window.feather) window.feather.replace(); } catch (e) {}

    // 回复语义（对齐 TypechoComment.reply）：把表单搬到被回评论下，设 parent，显示取消。
    var origParent = null;
    window.TypechoComment = {
      dom: function (id) { return document.getElementById(id); },
      reply: function (cid, coid) {
        var comment = this.dom(cid);
        var response = document.querySelector('.comment-respond');
        var input = document.getElementById('comment-parent');
        if (!comment || !response) return true;
        if (origParent === null && input) origParent = input.value;
        if (!this.dom('comment-form-place-holder')) {
          var holder = document.createElement('div');
          holder.id = 'comment-form-place-holder';
          response.parentNode.insertBefore(holder, response);
        }
        comment.appendChild(response);
        if (input) input.value = String(coid);
        var cancel = document.getElementById('cancel-comment-reply-link');
        if (cancel) cancel.style.display = '';
        var ta = response.querySelector('textarea');
        if (ta) ta.focus();
        return false;
      },
      cancelReply: function () {
        var holder = this.dom('comment-form-place-holder');
        var response = document.querySelector('.comment-respond');
        var input = document.getElementById('comment-parent');
        if (input && origParent !== null) input.value = origParent;
        if (holder && response) holder.parentNode.insertBefore(response, holder);
        var cancel = document.getElementById('cancel-comment-reply-link');
        if (cancel) cancel.style.display = 'none';
        return false;
      }
    };
    var cancelLink = document.getElementById('cancel-comment-reply-link');
    if (cancelLink) cancelLink.addEventListener('click', function (ev) { ev.preventDefault(); window.TypechoComment.cancelReply(); });
    // PJAX 换页后重渲行内图标（评论区心形等）。
    document.addEventListener('pjax:end', function () { try { if (window.feather) window.feather.replace(); } catch (e) {} });

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

    // 右栏图标页签（现网 widget-tabs）：点图标切热门/评论/随机。
    document.querySelectorAll('#right_first_section .tablist a[data-target]').forEach(function (tab) {
      tab.addEventListener('click', function (ev) {
        ev.preventDefault();
        var section = tab.closest('section');
        if (!section) return;
        section.querySelectorAll('.tablist li').forEach(function (li) { li.classList.remove('active'); });
        section.querySelectorAll('.tab-pane').forEach(function (pane) { pane.classList.remove('active'); });
        if (tab.parentElement) tab.parentElement.classList.add('active');
        var pane = section.querySelector(tab.getAttribute('data-target'));
        if (pane) pane.classList.add('active');
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
    // 说说私密开关：提交时把正文包进 [secret]…[/secret]（渲染侧按登录态展示）。
    var sayForm = document.getElementById('say_form');
    if (sayForm) sayForm.addEventListener('submit', function () {
      var box = document.getElementById('secret_comment_checkbox');
      var ta = sayForm.querySelector('textarea[name="text"]');
      if (box && box.checked && ta && ta.value.indexOf('[secret]') !== 0) {
        ta.value = '[secret]' + ta.value + '[/secret]';
      }
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

    // 自适应字色：按面板背后有效亮度选深/浅字（相对亮度 + 对比度阈值）。
    // 背景为 fixed cover，取样按视口坐标；再与玻璃 tint 做 over 合成后算亮度。
    (function adaptiveInk() {
      var SELECTORS = ['#header.app-header', '#aside.app-aside', '.single-post.panel', '#footer.app-footer', '#search_modal'];
      var DARK = { ink: '#1c2333', soft: '#4a5570', link: '#0a58ca' };
      var LIGHT = { ink: '#f2f5fb', soft: '#c3cbdd', link: '#9db9ff' };
      var THRESHOLD = 0.55;
      var canvas = null;
      var view = { w: 0, h: 0 };
      var timer = 0;

      function srgbToLinear(c) {
        c /= 255;
        return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
      }
      function luminance(r, g, b) {
        return 0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);
      }
      function contrast(l1, l2) {
        var a = Math.max(l1, l2), b = Math.min(l1, l2);
        return (a + 0.05) / (b + 0.05);
      }
      function parseRgba(s) {
        var m = /rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)(?:\s*,\s*([\d.]+))?\s*\)/.exec(s || '');
        if (!m) return null;
        return { r: +m[1], g: +m[2], b: +m[3], a: m[4] === undefined ? 1 : +m[4] };
      }
      function bgImageUrl() {
        try {
          var bg = getComputedStyle(document.body).backgroundImage || '';
          var urls = [];
          bg.replace(/url\(("([^"]+)"|'([^']+)'|([^\)]+))\)/g, function (_, __, a, b, c) {
            urls.push((a || b || c || '').trim());
          });
          for (var i = 0; i < urls.length; i++) {
            if (/glass-bg/i.test(urls[i])) return urls[i];
          }
          return urls[0] || '';
        } catch (e) { return ''; }
      }
      function ensureCanvas(img) {
        var vw = window.innerWidth, vh = window.innerHeight;
        if (!vw || !vh) return false;
        var k = Math.min(1, 320 / vw);
        var w = Math.max(1, Math.round(vw * k)), h = Math.max(1, Math.round(vh * k));
        if (!canvas) canvas = document.createElement('canvas');
        canvas.width = w; canvas.height = h;
        var ctx = canvas.getContext('2d');
        if (!ctx) return false;
        // cover + center：与 CSS `center / cover fixed` 同算法。
        var s = Math.max(w / img.naturalWidth, h / img.naturalHeight);
        var dw = img.naturalWidth * s, dh = img.naturalHeight * s;
        ctx.drawImage(img, (w - dw) / 2, (h - dh) / 2, dw, dh);
        view = { w: w, h: h, k: k };
        return true;
      }
      function samplePanel(el) {
        try {
          var ctx = canvas.getContext('2d');
          var r = el.getBoundingClientRect();
          var x0 = Math.max(0, r.left * view.k), y0 = Math.max(0, r.top * view.k);
          var x1 = Math.min(view.w, r.right * view.k), y1 = Math.min(view.h, r.bottom * view.k);
          if (x1 <= x0 || y1 <= y0) return null;
          var n = 5, rs = 0, gs = 0, bs = 0, count = 0;
          for (var i = 0; i < n; i++) {
            for (var j = 0; j < n; j++) {
              var x = Math.floor(x0 + (x1 - x0) * (i + 0.5) / n);
              var y = Math.floor(y0 + (y1 - y0) * (j + 0.5) / n);
              var d = ctx.getImageData(x, y, 1, 1).data;
              rs += d[0]; gs += d[1]; bs += d[2]; count++;
            }
          }
          return { r: rs / count, g: gs / count, b: bs / count };
        } catch (e) { return null; }
      }
      function applyFallback() {
        var light = document.body.classList.contains('light');
        document.querySelectorAll(SELECTORS.join(',')).forEach(function (el) {
          setPalette(el, light ? 'dark' : 'light');
        });
      }
      function setPalette(el, which) {
        var p = which === 'dark' ? DARK : LIGHT;
        el.style.setProperty('--ink', p.ink);
        el.style.setProperty('--ink-soft', p.soft);
        el.style.setProperty('--link', p.link);
        el.setAttribute('data-ink', which);
      }
      function recalc() {
        if (!canvas) { applyFallback(); return; }
        document.querySelectorAll(SELECTORS.join(',')).forEach(function (el) {
          if (el.classList.contains('hide')) return;
          var s = samplePanel(el);
          if (!s) return;
          // 叠玻璃 tint 再算亮度。
          var tint = parseRgba(getComputedStyle(el).getPropertyValue('--glass-tint')) || { r: 255, g: 255, b: 255, a: 0.18 };
          var r = tint.r * tint.a + s.r * (1 - tint.a);
          var g = tint.g * tint.a + s.g * (1 - tint.a);
          var b = tint.b * tint.a + s.b * (1 - tint.a);
          var leff = luminance(r, g, b);
          var cDark = contrast(leff, luminance(28, 35, 51));
          var cLight = contrast(leff, luminance(242, 245, 251));
          // 亮底优先深字（达标或更优），暗底优先浅字。
          var useDark = leff >= THRESHOLD ? (cDark >= 4.5 || cDark >= cLight) : (cLight >= 4.5 ? false : cDark > cLight);
          setPalette(el, useDark ? 'dark' : 'light');
        });
      }
      function schedule() {
        if (timer) clearTimeout(timer);
        timer = setTimeout(recalc, 180);
      }
      function boot() {
        var url = bgImageUrl();
        if (!url) { applyFallback(); return; }
        var img = new Image();
        img.onload = function () {
          try {
            if (ensureCanvas(img)) recalc();
            else applyFallback();
          } catch (e) { applyFallback(); }
        };
        img.onerror = applyFallback;
        img.src = url;
      }
      if (toggle) toggle.addEventListener('click', function () { schedule(); });
      window.addEventListener('resize', schedule);
      window.addEventListener('scroll', schedule, { passive: true });
      document.addEventListener('pjax:end', schedule);
      boot();
    })();

    // 日历/雷达占位：有数据则列出入口，无 ECharts 依赖。
    // 行式与主 Categories 组一致：名称 + 右侧独立 badge 块，不用浏览器默认圆点/文案计数。
    function fill(id, items, fmt) {
      var el = document.getElementById(id);
      if (!el || !items || !items.length) return;
      el.classList.remove('hide');
      var ul = document.createElement('ul');
      ul.className = 'nav nav-sub dk';
      items.slice(0, 24).forEach(function (it) {
        var f = fmt(it);
        var li = document.createElement('li');
        var a = document.createElement('a');
        a.href = it.url;
        if (f && typeof f === 'object') {
          if (f.badge != null) {
            var b = document.createElement('b');
            b.className = 'badge pull-right';
            b.textContent = f.badge;
            a.appendChild(b);
          }
          a.appendChild(document.createTextNode(f.label));
        } else {
          a.textContent = f;
        }
        li.appendChild(a); ul.appendChild(li);
      });
      el.appendChild(ul);
    }
    fill('post-calendar', TB.CALENDAR, function (it) { return it.year + '-' + it.month; });
    fill('category-radar', TB.CATS, function (it) { return { label: it.name, badge: it.count }; });
  });
})();
