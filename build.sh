#!/bin/sh
# Build script - bundles CSS (inlined into HTML) and JS (external, deferred).
# CSS is inlined to eliminate render-blocking requests.
# JS is external with defer so it doesn't block first paint.

set -e

#    CSS bundle   
echo "Bundling CSS..."
cat frontend/styles/tokens.css \
    frontend/styles/base.css \
    frontend/styles/layout.css \
    frontend/styles/components.css \
    frontend/styles/modal.css \
    frontend/styles/log.css \
    frontend/styles/capture.css \
    frontend/styles/toast.css \
    frontend/styles/animations.css \
    > frontend/bundle.css

#    JS bundle (content-hash fingerprinted for cache busting)   
# The bundle URL includes a SHA-256 hash of its content: the URL changes
# iff the content changes, so the browser can cache it as immutable forever.
# On rebuild, a new hash → new URL → browser fetches fresh JS automatically.
echo "Bundling JS..."

MODULES="
frontend/js/state/store.js
frontend/js/state/dom.js
frontend/js/utils/format.js
frontend/js/utils/form.js
frontend/js/utils/dom-helpers.js
frontend/js/utils/buttons.js
frontend/js/utils/net.js
frontend/js/components/toast.js
frontend/js/components/confirm.js
frontend/js/components/modal.js
frontend/js/components/goal-form.js
frontend/js/data/client-schema.js
frontend/js/data/capture-helpers.js
frontend/js/data/labels.js
frontend/js/services/clients.js
frontend/js/services/runtime.js
frontend/js/components/client-card.js
frontend/js/components/task-list.js
frontend/js/components/log-panel.js
frontend/js/components/task-modal.js
frontend/js/components/settings-modal.js
frontend/js/components/goals-modal.js
frontend/js/components/capture-modal.js
frontend/js/services/sse.js
frontend/js/app/init.js
"

{
  echo "// Auto-generated bundle - do not edit. Run ./build.sh to regenerate."
  for mod in $MODULES; do
    echo ""
    sed -e "/^import /d" \
        -e "s/^export async function/async function/" \
        -e "s/^export function/function/" \
        -e "s/^export const/const/" \
        -e "s/^export let/let/" \
        -e "s/^export class/class/" \
        -e "/^export {/d" \
        "$mod"
  done
  echo ""
  echo 'if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", init);'
  echo 'else init();'
} > frontend/.bundle.tmp.js

# Compute content hash, clean up old fingerprinted bundles, rename to final.
BUNDLE_HASH=$(sha256sum frontend/.bundle.tmp.js | cut -c1-12)
BUNDLE_NAME="bundle.${BUNDLE_HASH}.js"
for f in frontend/bundle.*.js; do
  [ -f "$f" ] && [ "$f" != "frontend/${BUNDLE_NAME}" ] && rm -- "$f"
done
mv frontend/.bundle.tmp.js "frontend/${BUNDLE_NAME}"
echo "Bundle fingerprint: ${BUNDLE_NAME}"

#    Inline CSS into index.html, keep JS external with defer   
echo "Inlining CSS into index.html..."
BUNDLE_NAME="$BUNDLE_NAME" python3 -c "
import os, re
with open('frontend/bundle.css') as f:
    css = f.read()
with open('templates/index.html') as f:
    html = f.read()
# Inline CSS. Two paths so a re-run after a CSS edit refreshes the inlined
# <style> (the CSS source is the bundle, not the link tag, once inlined):
#   - fresh build: replace the <link> with a <style> block
#   - re-build:    replace the existing inlined <style> with the new CSS
link = '<link rel=\"stylesheet\" href=\"/static/bundle.css\">'
if link in html:
    html = html.replace(link, '<style>' + css + '</style>')
else:
    html = re.sub(r'<style>.*?</style>', '<style>' + css + '</style>', html, count=1, flags=re.DOTALL)
# Wire the deferred bundle (content-hash fingerprinted). Three paths, all idempotent:
#   1. re-build with existing fingerprinted tag → replace with new hash
#   2. re-build with old non-fingerprinted tag  → replace with fingerprinted
#   3. fresh build (dev inline <script> entry)   → replace first non-bootstrap script
bundle_name = os.environ['BUNDLE_NAME']
bundle_url = '/static/' + bundle_name
fingerprinted = re.compile(r'<script defer src=\"/static/bundle\.[a-f0-9]+\.js\"></script>')
if fingerprinted.search(html):
    html = fingerprinted.sub('<script defer src=\"' + bundle_url + '\"></script>', html)
elif '<script defer src=\"/static/bundle.js\"></script>' in html:
    html = html.replace('<script defer src=\"/static/bundle.js\"></script>',
                        '<script defer src=\"' + bundle_url + '\"></script>')
else:
    for m in re.finditer(r'<script\b[^>]*>.*?</script>', html, flags=re.DOTALL):
        if '__BOOTSTRAP__' in m.group(0):
            continue
        html = html[:m.start()] + '<script defer src=\"' + bundle_url + '\"></script>' + html[m.end():]
        break
with open('templates/index.html', 'w') as f:
    f.write(html)
"

echo "Done. index.html: $(wc -c < templates/index.html) bytes, ${BUNDLE_NAME}: $(wc -c < "frontend/${BUNDLE_NAME}") bytes (gzip: $(gzip -c "frontend/${BUNDLE_NAME}" | wc -c) bytes)"
