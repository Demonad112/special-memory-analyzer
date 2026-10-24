;(function () {
  'use strict'

  var REPO = 'Demonad112/special-memory-analyzer'
  var RELEASES = 'https://github.com/' + REPO + '/releases'

  // ---------- theme ----------
  var root = document.documentElement
  function storedTheme() {
    try {
      return localStorage.getItem('dsa-theme')
    } catch (e) {
      return null
    }
  }
  function applyTheme(t) {
    if (t === 'light' || t === 'dark') root.setAttribute('data-theme', t)
    else root.removeAttribute('data-theme')
  }
  applyTheme(storedTheme())

  var toggle = document.getElementById('theme-toggle')
  if (toggle) {
    toggle.addEventListener('click', function () {
      var dark =
        root.getAttribute('data-theme') === 'dark' ||
        (!root.getAttribute('data-theme') &&
          window.matchMedia('(prefers-color-scheme: dark)').matches)
      var next = dark ? 'light' : 'dark'
      applyTheme(next)
      try {
        localStorage.setItem('dsa-theme', next)
      } catch (e) {
        /* storage blocked: theme just applies for this visit */
      }
    })
  }

  // ---------- latest release box ----------
  function fmtSize(n) {
    if (n >= 1048576) return (n / 1048576).toFixed(1) + ' MB'
    return Math.max(1, Math.round(n / 1024)) + ' KB'
  }

  var ASSETS = [
    {
      test: /_x64-setup\.exe$/i,
      label: 'Windows installer',
      desc: 'Per-machine NSIS installer. Windows 10/11 and Server 2016+, 64-bit.',
      primary: true,
    },
    {
      test: /usb-kit\.zip$/i,
      label: 'USB kit for servers',
      desc: 'Installs on locked-down servers with time-limited certificate trust.',
    },
    {
      test: /^dsa-signing\.cer$/i,
      label: 'Signing certificate',
      desc: 'Public certificate. Compare its thumbprint with the one below.',
    },
    {
      test: /^SHA256SUMS-release\.txt$/i,
      label: 'SHA-256 of installer and kit',
      desc: 'Check the download hash before you run it.',
    },
  ]

  function el(tag, cls, text) {
    var n = document.createElement(tag)
    if (cls) n.className = cls
    if (text != null) n.textContent = text
    return n
  }

  function render(box, rel) {
    var list = box.querySelector('[data-dl-list]')
    var ver = box.querySelector('[data-dl-version]')
    var meta = box.querySelector('[data-dl-meta]')
    var thumb = box.querySelector('[data-dl-thumb]')
    if (!list || !ver) return

    ver.textContent = rel.tag_name
    if (meta) {
      var d = new Date(rel.published_at)
      meta.textContent = isNaN(d) ? '' : 'Released ' + d.toISOString().slice(0, 10)
    }

    list.textContent = ''
    ASSETS.forEach(function (spec) {
      var a = (rel.assets || []).filter(function (x) {
        return spec.test.test(x.name)
      })[0]
      if (!a) return
      var li = el('li')
      li.appendChild(el('span', 'name', spec.label + ' · ' + fmtSize(a.size)))
      li.appendChild(el('span', 'desc', a.name))
      var link = el('a', 'btn sm' + (spec.primary ? ' primary' : ''), 'Download')
      link.href = a.browser_download_url
      link.setAttribute('aria-label', 'Download ' + a.name)
      li.appendChild(link)
      list.appendChild(li)
    })

    if (!list.children.length) {
      var li2 = el('li')
      li2.appendChild(el('span', 'name', 'Files are on the release page'))
      var l2 = el('a', 'btn sm primary', 'Open release')
      l2.href = rel.html_url
      li2.appendChild(l2)
      list.appendChild(li2)
    }

    var m = /Thumbprint \(SHA-1\)[^0-9A-Fa-f]*([0-9A-Fa-f]{40})/.exec(rel.body || '')
    if (thumb && m) {
      var code = thumb.querySelector('code')
      if (code) {
        code.textContent = m[1].toUpperCase()
        thumb.hidden = false
      }
    }
    box.setAttribute('data-state', 'ready')
  }

  function fail(box) {
    var ver = box.querySelector('[data-dl-version]')
    var meta = box.querySelector('[data-dl-meta]')
    var list = box.querySelector('[data-dl-list]')
    if (ver) ver.textContent = 'Latest'
    if (meta) meta.textContent = 'Could not read release details. Use the release page.'
    if (list) {
      list.textContent = ''
      var li = el('li')
      li.appendChild(el('span', 'name', 'Installer, USB kit and checksums'))
      var a = el('a', 'btn sm primary', 'Open latest release')
      a.href = RELEASES + '/latest'
      li.appendChild(a)
      list.appendChild(li)
    }
    box.setAttribute('data-state', 'fallback')
  }

  var box = document.querySelector('[data-download]')
  if (box && window.fetch) {
    fetch('https://api.github.com/repos/' + REPO + '/releases/latest', {
      headers: { Accept: 'application/vnd.github+json' },
    })
      .then(function (r) {
        if (!r.ok) throw new Error('HTTP ' + r.status)
        return r.json()
      })
      .then(function (rel) {
        render(box, rel)
      })
      .catch(function () {
        fail(box)
      })
  }

  // ---------- table of contents highlight ----------
  var toc = document.querySelectorAll('.toc a[href^="#"]')
  if (toc.length && 'IntersectionObserver' in window) {
    var map = {}
    toc.forEach(function (a) {
      map[a.getAttribute('href').slice(1)] = a
    })
    var io = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (e) {
          if (e.isIntersecting && map[e.target.id]) {
            toc.forEach(function (a) {
              a.classList.remove('on')
            })
            map[e.target.id].classList.add('on')
          }
        })
      },
      { rootMargin: '-80px 0px -65% 0px' },
    )
    Object.keys(map).forEach(function (id) {
      var s = document.getElementById(id)
      if (s) io.observe(s)
    })
  }

  // ---------- FAQ search / expand ----------
  var q = document.getElementById('faq-search')
  var items = document.querySelectorAll('details[data-faq]')
  if (q && items.length) {
    q.addEventListener('input', function () {
      var t = q.value.trim().toLowerCase()
      items.forEach(function (d) {
        var hit = !t || d.textContent.toLowerCase().indexOf(t) !== -1
        d.hidden = !hit
        if (t && hit) d.open = true
      })
      document.querySelectorAll('.faq-group').forEach(function (g) {
        var any = g.querySelectorAll('details[data-faq]:not([hidden])').length
        g.hidden = !any
      })
      var none = document.getElementById('faq-none')
      if (none) none.hidden = !!document.querySelector('details[data-faq]:not([hidden])')
    })
    var all = document.getElementById('faq-toggle')
    if (all) {
      all.addEventListener('click', function () {
        var open = all.getAttribute('data-open') !== '1'
        items.forEach(function (d) {
          if (!d.hidden) d.open = open
        })
        all.setAttribute('data-open', open ? '1' : '0')
        all.textContent = open ? 'Collapse all' : 'Expand all'
      })
    }
    // open a question linked by #hash
    if (location.hash) {
      var target = document.querySelector(location.hash)
      if (target && target.tagName === 'DETAILS') target.open = true
    }
  }
})()
