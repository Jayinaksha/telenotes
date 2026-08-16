// Markdown + LaTeX rendering for note previews. The libraries are loaded on
// first use via dynamic import, not at page load -- most sessions browse
// without ever rendering a note, so there's no reason to pay for marked,
// katex, and dompurify on every visit.

let libs = null;

async function load() {
  if (!libs) {
    const [{ marked }, katexModule, dompurifyModule] = await Promise.all([
      import("https://esm.sh/marked@13"),
      import("https://esm.sh/katex@0.16.9"),
      import("https://esm.sh/dompurify@3"),
    ]);
    libs = { marked, katex: katexModule.default, DOMPurify: dompurifyModule.default };
  }
  return libs;
}

const MATH_RE = /\$\$([\s\S]+?)\$\$|\$([^\n$]+?)\$/g;

/// Renders markdown with inline (`$..$`) and display (`$$..$$`) LaTeX math
/// to sanitized HTML. Math is pulled out and rendered by KaTeX first --
/// otherwise markdown's own escaping (underscores, asterisks) mangles TeX
/// source -- then spliced back in after the markdown pass.
export async function renderNoteHtml(text) {
  const { marked, katex, DOMPurify } = await load();

  const math = [];
  const withPlaceholders = text.replace(MATH_RE, (match, display, inline) => {
    const tex = display ?? inline;
    let html;
    try {
      html = katex.renderToString(tex, { throwOnError: false, displayMode: Boolean(display) });
    } catch {
      html = DOMPurify.sanitize(match);
    }
    math.push(html);
    return `@@TELENOTES-MATH-${math.length - 1}@@`;
  });

  const rendered = marked.parse(withPlaceholders, { breaks: true });
  const withMath = rendered.replace(/@@TELENOTES-MATH-(\d+)@@/g, (_, i) => math[Number(i)]);
  return DOMPurify.sanitize(withMath);
}

/// Sanitizes an uploaded HTML document for the sandboxed preview iframe.
/// `WHOLE_DOCUMENT` keeps the doctype/html/head/body structure intact
/// (unlike `renderNoteHtml`'s fragment output, this goes into an iframe's
/// `srcdoc`, not `innerHTML` on an existing element, so there's a real head
/// to preserve). Sanitization here is defense-in-depth, not the primary
/// guard -- the iframe's empty `sandbox` attribute is what actually stops
/// anything that gets through from executing.
export async function sanitizeHtml(html) {
  const { DOMPurify } = await load();
  return DOMPurify.sanitize(html, { WHOLE_DOCUMENT: true });
}
