/* Snapshot, probe, and metadata code for the bob web-clip adapter.
 *
 * Loaded by web_clip_adapter.py via page.evaluate(), which defines
 * window.__bobClip = { probe, snapshot }. No dependencies; plain script
 * (not a module) so it runs under any page CSP.
 *
 * probe() returns raw page facts; classification lives in Python
 * (classify_probe) so it stays unit-testable.
 *
 * snapshot() clones the live documentElement, cleans the clone while
 * walking live and clone elements in parallel, and returns the cleaned
 * HTML plus metadata/fidelity facts recorded during the walk.
 */
(function () {
  "use strict";

  var EMBED_SELECTORS = [
    "blockquote.twitter-tweet",
    "[data-testid]",
    "iframe",
  ];

  function hasClassToken(el, token) {
    var cls = (el.getAttribute && el.getAttribute("class")) || "";
    return cls.toLowerCase().split(/\s+/).indexOf(token) !== -1;
  }

  function isEmbed(el) {
    for (var node = el; node && node.nodeType === 1; node = node.parentElement) {
      var tag = (node.tagName || "").toLowerCase();
      if (tag === "iframe") return true;
      if (tag === "blockquote" && hasClassToken(node, "twitter-tweet")) return true;
      var cls = (node.getAttribute && node.getAttribute("class")) || "";
      var low = cls.toLowerCase();
      if (low.indexOf("tweet") !== -1 || low.indexOf("embed") !== -1) return true;
      var testid = node.getAttribute && node.getAttribute("data-testid");
      if (testid && testid.toLowerCase().indexOf("tweet") !== -1) return true;
    }
    return false;
  }

  function visibleWords(root) {
    var text = (root && root.innerText) || "";
    var words = text.split(/\s+/).filter(function (w) { return w.length > 0; });
    return { count: words.length, text: text };
  }

  var CHALLENGE_TITLES = [
    "just a moment",
    "attention required",
    "checking your browser",
    "access denied",
  ];

  var LOGIN_PHRASES = [
    "sign in to continue",
    "log in to continue",
    "subscribe to continue reading",
    "create a free account",
  ];

  function probe() {
    var title = document.title || "";
    var lowered = title.trim().toLowerCase();
    var challengeTitle = CHALLENGE_TITLES.some(function (t) {
      return lowered.indexOf(t) === 0;
    });
    var challengeDom =
      challengeTitle ||
      !!document.querySelector(
        "#challenge-error-text, #cf-challenge-running, form#challenge-form"
      ) ||
      window.__bobClipCfMitigated === true;
    var body = visibleWords(document.body);
    var low = body.text.toLowerCase();
    var loginPhrases = LOGIN_PHRASES.filter(function (p) {
      return low.indexOf(p) !== -1;
    });
    var articleCandidate = !!document.querySelector("h1, article, main");
    return {
      title: title,
      challengeDom: challengeDom,
      loginPhrases: loginPhrases,
      articleCandidate: articleCandidate,
      visibleWords: body.count,
    };
  }

  function firstH1() {
    var h1s = document.getElementsByTagName("h1");
    for (var i = 0; i < h1s.length; i++) {
      if (!isEmbed(h1s[i])) return h1s[i];
    }
    return h1s.length > 0 ? h1s[0] : null;
  }

  var DATE_RE = new RegExp(
    "(?:(?:January|February|March|April|May|June|July|August|September|" +
      "October|November|December|Jan|Feb|Mar|Apr|Jun|Jul|Aug|Sep|Sept|" +
      "Oct|Nov|Dec)\\.?\\s+\\d{1,2},?\\s+\\d{4}|" +
      "\\d{1,2}\\s+(?:January|February|March|April|May|June|July|August|" +
      "September|October|November|December)\\s+\\d{4}|" +
      "\\d{4}-\\d{2}-\\d{2})"
  );

  function readJsonLd() {
    var author = null;
    var datePublished = null;
    var scripts = document.querySelectorAll('script[type="application/ld+json"]');
    function visit(node) {
      if (!node || author !== null && datePublished !== null) return;
      if (Array.isArray(node)) {
        node.forEach(visit);
        return;
      }
      if (typeof node !== "object") return;
      var type = node["@type"];
      var types = Array.isArray(type) ? type : [type];
      var isArticle = types.some(function (t) {
        return t === "Article" || t === "BlogPosting" || t === "NewsArticle";
      });
      var graph = node["@graph"];
      if (graph) visit(graph);
      if (!isArticle) {
        Object.keys(node).forEach(function (k) {
          if (k.charAt(0) !== "@") visit(node[k]);
        });
        return;
      }
      if (author === null && node.author) {
        var a = node.author;
        if (Array.isArray(a)) a = a[0];
        if (typeof a === "string") author = a;
        else if (a && typeof a.name === "string") author = a.name;
      }
      if (datePublished === null && typeof node.datePublished === "string") {
        datePublished = node.datePublished;
      }
    }
    for (var i = 0; i < scripts.length; i++) {
      var el = scripts[i];
      if (isEmbed(el)) continue;
      try {
        visit(JSON.parse(el.textContent || "null"));
      } catch (e) {
        /* ignore malformed JSON-LD blocks */
      }
    }
    return { author: author, datePublished: datePublished };
  }

  function metaContent(names) {
    for (var i = 0; i < names.length; i++) {
      var kind = names[i][0];
      var value = names[i][1];
      var el =
        kind === "name"
          ? document.querySelector('meta[name="' + value + '"]')
          : document.querySelector('meta[property="' + value + '"]');
      if (el && el.getAttribute("content")) return el.getAttribute("content");
    }
    return null;
  }

  function nearH1(h1) {
    var dates = [];
    var byline = null;
    var seen = {};
    function considerDate(raw) {
      if (!raw || seen[raw]) return;
      seen[raw] = true;
      dates.push(raw);
    }
    if (!h1) return { dates: dates, byline: byline };
    var scope = h1;
    for (var level = 0; level < 3 && scope; level++) {
      var times = scope.querySelectorAll
        ? scope.querySelectorAll("time[datetime]")
        : [];
      for (var i = 0; i < times.length; i++) {
        if (isEmbed(times[i])) continue;
        considerDate(times[i].getAttribute("datetime"));
      }
      scope = scope.parentElement;
    }
    // Date-like text nodes near the H1: every visible text node inside the
    // H1's ancestors up to three levels, nearest scope first, skipping
    // scripts, styles, and embeds (data islands carry unrelated dates).
    var up = h1;
    for (var depth = 0; depth < 3 && up; depth++) {
      var walker = document.createTreeWalker(up, NodeFilter.SHOW_TEXT, null);
      var node;
      while ((node = walker.nextNode())) {
        var parent = node.parentElement;
        if (!parent) continue;
        var tag = (parent.tagName || "").toUpperCase();
        if (tag === "SCRIPT" || tag === "STYLE") continue;
        if (isEmbed(parent)) continue;
        var m = DATE_RE.exec(node.nodeValue || "");
        if (m) considerDate(m[0]);
      }
      up = up.parentElement;
    }
    // Byline: "By Name" text near the H1 (siblings and parent text).
    var pools = [];
    if (h1.parentElement) pools.push(h1.parentElement);
    if (h1.parentElement && h1.parentElement.parentElement) {
      pools.push(h1.parentElement.parentElement);
    }
    for (var p = 0; p < pools.length && byline === null; p++) {
      var walker = document.createTreeWalker(
        pools[p],
        NodeFilter.SHOW_TEXT,
        null
      );
      var node;
      while ((node = walker.nextNode())) {
        if (isEmbed(node.parentElement)) continue;
        var m2 = /^\s*By\s+(.+?)\s*$/.exec(node.nodeValue || "");
        if (m2 && m2[1].length > 1 && m2[1].length < 200) {
          byline = m2[1];
          break;
        }
      }
    }
    return { dates: dates, byline: byline };
  }

  function embedFacts() {
    var authors = [];
    var dates = [];
    var blocks = document.querySelectorAll(
      "blockquote.twitter-tweet, [class*=tweet], [data-testid]"
    );
    for (var i = 0; i < blocks.length; i++) {
      var el = blocks[i];
      if (!isEmbed(el)) continue;
      var links = el.querySelectorAll("a[href]");
      for (var j = 0; j < links.length; j++) {
        var href = links[j].getAttribute("href") || "";
        var hm = /(?:twitter\.com|x\.com)\/([A-Za-z0-9_]{1,15})/.exec(href);
        if (hm) authors.push("@" + hm[1]);
        var text = (links[j].textContent || "").trim();
        if (/^@/.test(text) && text.length < 30) authors.push(text);
      }
      var named = el.querySelectorAll("[data-screen-name]");
      for (var k = 0; k < named.length; k++) {
        authors.push("@" + named[k].getAttribute("data-screen-name"));
      }
      var times = el.querySelectorAll("time[datetime]");
      for (var t = 0; t < times.length; t++) {
        dates.push(times[t].getAttribute("datetime"));
      }
    }
    return { authors: authors, dates: dates };
  }

  function hiddenInLive(el) {
    if (el.getAttribute && el.getAttribute("aria-hidden") === "true") {
      return el.closest && el.closest("figure") ? false : true;
    }
    var style;
    try {
      style = window.getComputedStyle(el);
    } catch (e) {
      return false;
    }
    if (!style) return false;
    if (style.display === "none" || style.visibility === "hidden") return true;
    // Screen-reader-only boxes: 1px or smaller with clipped/hidden overflow.
    var rect = null;
    try {
      rect = el.getBoundingClientRect();
    } catch (e) {
      return false;
    }
    if (rect && rect.width <= 1 && rect.height <= 1) {
      var overflow = (style.overflow || "") + " " + (style.overflowX || "");
      var clip = style.clip || "";
      if (
        overflow.indexOf("hidden") !== -1 ||
        overflow.indexOf("clip") !== -1 ||
        clip.indexOf("rect(") !== -1 ||
        style.clipPath === "inset(50%)"
      ) {
        return true;
      }
    }
    return false;
  }

  var REMOVE_TAGS = {
    SCRIPT: true,
    STYLE: true,
    LINK: true,
    TEMPLATE: true,
    NOSCRIPT: true,
    IFRAME: true,
    FORM: true,
    INPUT: true,
    BUTTON: true,
    SELECT: true,
    TEXTAREA: true,
  };

  var SUPPORTED_IMAGE_TYPES = {
    "image/jpeg": true,
    "image/png": true,
    "image/gif": true,
    "image/webp": true,
    "image/avif": true,
    "image/svg+xml": true,
  };

  function parseSrcset(srcset) {
    var out = [];
    var parts = (srcset || "").split(",");
    for (var i = 0; i < parts.length; i++) {
      var tokens = parts[i].trim().split(/\s+/).filter(Boolean);
      if (tokens.length === 0) continue;
      var url = tokens[0];
      var desc = tokens[1] || "";
      var w = null;
      var d = null;
      var m;
      if ((m = /^(\d+)w$/.exec(desc))) w = parseInt(m[1], 10);
      else if ((m = /^([\d.]+)x$/.exec(desc))) d = parseFloat(m[1]);
      out.push({ url: url, w: w, d: d });
    }
    return out;
  }

  function bestSrcsetUrl(srcset) {
    var cands = parseSrcset(srcset);
    if (cands.length === 0) return null;
    var withW = cands.filter(function (c) { return c.w !== null; });
    if (withW.length > 0) {
      withW.sort(function (a, b) { return a.w - b.w; });
      for (var i = withW.length - 1; i >= 0; i--) {
        if (withW[i].w <= 2560) return withW[i].url;
      }
      return withW[withW.length - 1].url;
    }
    var withD = cands.filter(function (c) { return c.d !== null; });
    if (withD.length > 0) {
      withD.sort(function (a, b) { return a.d - b.d; });
      return withD[withD.length - 1].url;
    }
    return cands[cands.length - 1].url;
  }

  function sourceTypeOk(source) {
    var type = (source.getAttribute("type") || "").trim().toLowerCase();
    if (!type) return true;
    return !!SUPPORTED_IMAGE_TYPES[type.split(";")[0].trim()];
  }

  function resolvePicture(pictureLive, imgClone, base) {
    var sources = pictureLive.getElementsByTagName("source");
    var picked = null;
    for (var i = 0; i < sources.length; i++) {
      var media = sources[i].getAttribute("media");
      var matches = true;
      if (media) {
        try {
          matches = window.matchMedia(media).matches;
        } catch (e) {
          matches = true;
        }
      }
      if (matches && sourceTypeOk(sources[i])) {
        picked = sources[i];
        break;
      }
    }
    var src = null;
    if (picked) {
      src =
        bestSrcsetUrl(
          picked.getAttribute("srcset") || picked.getAttribute("data-srcset")
        ) || picked.getAttribute("src");
    }
    if (!src) {
      var liveImgs = pictureLive.getElementsByTagName("img");
      var liveImg = liveImgs.length > 0 ? liveImgs[0] : null;
      if (liveImg) {
        src = promoteImageSrc(liveImg);
      }
    }
    if (src) {
      try {
        imgClone.setAttribute("src", new URL(src, base).toString());
      } catch (e) {
        imgClone.setAttribute("src", src);
      }
    }
    var liveImg0 = pictureLive.getElementsByTagName("img")[0];
    if (liveImg0) {
      ["alt", "width", "height"].forEach(function (attr) {
        var v = liveImg0.getAttribute(attr);
        if (v !== null && !imgClone.getAttribute(attr)) {
          imgClone.setAttribute(attr, v);
        }
      });
    }
    imgClone.removeAttribute("srcset");
    imgClone.removeAttribute("sizes");
    imgClone.removeAttribute("loading");
  }

  function promoteImageSrc(liveImg) {
    if (liveImg.currentSrc) return liveImg.currentSrc;
    var dataSet =
      liveImg.getAttribute("data-srcset") || liveImg.getAttribute("data-src");
    if (dataSet) {
      if (liveImg.getAttribute("data-srcset")) {
        return bestSrcsetUrl(dataSet) || dataSet;
      }
      return dataSet;
    }
    var srcset = liveImg.getAttribute("srcset");
    if (srcset) return bestSrcsetUrl(srcset) || liveImg.getAttribute("src");
    return liveImg.getAttribute("src");
  }

  function snapshot() {
    var base = document.baseURI || location.href;
    var liveRoot = document.documentElement;
    var cloneRoot = liveRoot.cloneNode(true);
    var h1 = firstH1();
    var h1Top = null;
    if (h1) {
      try {
        h1Top = h1.getBoundingClientRect().top;
      } catch (e) {
        h1Top = null;
      }
    }
    var largeMedia = 0;
    // Count large media in the live page before cleaning.
    var media = liveRoot.querySelectorAll("picture, img");
    for (var mi = 0; mi < media.length; mi++) {
      var mel = media[mi];
      if (mel.tagName.toLowerCase() === "img" && mel.closest("picture")) continue;
      if (isEmbed(mel)) continue;
      var rect = null;
      try {
        rect = mel.getBoundingClientRect();
      } catch (e) {
        continue;
      }
      if (!rect || rect.width < 300) continue;
      if (h1Top !== null && rect.top <= h1Top) continue;
      largeMedia++;
    }
    var codeBlocks = 0;
    var pres = liveRoot.getElementsByTagName("pre");
    for (var pi = 0; pi < pres.length; pi++) {
      if ((pres[pi].textContent || "").trim().length > 0) codeBlocks++;
    }
    var words = visibleWords(document.body).count;

    // Walk live and clone in parallel; the clone mirrors the live tree
    // one-to-one because cloneNode preserves element order.
    var liveQueue = [liveRoot];
    var cloneQueue = [cloneRoot];
    var dropClones = [];
    while (liveQueue.length > 0) {
      var liveEl = liveQueue.shift();
      var cloneEl = cloneQueue.shift();
      var liveKids = liveEl.children || [];
      var cloneKids = cloneEl.children || [];
      for (var ci = 0; ci < liveKids.length; ci++) {
        var lk = liveKids[ci];
        var ck = cloneKids[ci];
        if (!ck) break;
        var tag = (lk.tagName || "").toUpperCase();
        if (REMOVE_TAGS[tag] || hiddenInLive(lk)) {
          dropClones.push(ck);
          continue;
        }
        if (tag === "PICTURE") {
          var imgClone = null;
          for (var q = 0; q < ck.children.length; q++) {
            if ((ck.children[q].tagName || "").toUpperCase() === "IMG") {
              imgClone = ck.children[q];
              break;
            }
          }
          if (imgClone) {
            resolvePicture(lk, imgClone, base);
            // The picture is resolved: keep only the winning <img> so no
            // dark-scheme <source> survives into extraction or print.
            ck.parentNode.insertBefore(imgClone, ck);
            ck.parentNode.removeChild(ck);
            continue;
          }
        } else if (tag === "IMG") {
          var promoted = promoteImageSrc(lk);
          if (promoted) {
            try {
              ck.setAttribute("src", new URL(promoted, base).toString());
            } catch (e) {
              ck.setAttribute("src", promoted);
            }
          }
          ck.removeAttribute("srcset");
          ck.removeAttribute("sizes");
          ck.removeAttribute("loading");
        } else if (tag === "SVG") {
          ck.setAttribute("data-bob-svg", "1");
        } else if (tag === "A") {
          var href = lk.getAttribute("href");
          if (href) {
            try {
              ck.setAttribute("href", new URL(href, base).toString());
            } catch (e) {
              ck.setAttribute("href", href);
            }
          }
        }
        liveQueue.push(lk);
        cloneQueue.push(ck);
      }
    }
    // Remove nested drops deepest-first so parents drop with children.
    dropClones.forEach(function (ck) {
      if (ck.parentNode) ck.parentNode.removeChild(ck);
    });

    var jsonLd = readJsonLd();
    var near = nearH1(h1);
    var embeds = embedFacts();
    var facts = {
      h1: h1 ? (h1.innerText || "").trim() : null,
      jsonLd: jsonLd,
      meta: {
        author: metaContent([["name", "author"]]),
        articleAuthor: metaContent([["property", "article:author"]]),
        publishedTime: metaContent([["property", "article:published_time"]]),
        siteName: metaContent([["property", "og:site_name"]]),
        ogTitle: metaContent([["property", "og:title"]]),
        description:
          metaContent([["property", "og:description"]]) ||
          metaContent([["name", "description"]]),
      },
      visibleDates: near.dates,
      byline: near.byline,
      embedAuthors: embeds.authors,
      embedDates: embeds.dates,
      counts: { largeMedia: largeMedia, codeBlocks: codeBlocks, words: words },
    };
    return { html: cloneRoot.outerHTML, facts: facts };
  }

  window.__bobClip = { probe: probe, snapshot: snapshot };
})();
