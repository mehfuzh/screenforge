"""Builds the Spanish site (site/es/) from the English pages.

Each Spanish page is the English page with its copy swapped through the tables
below, so layout, CSS and scripts never drift between the languages. Every
English string must match exactly once (or the count given): when a sentence on
an English page changes, this script fails on it and names the entry to
retranslate. It also puts the flag switch into the English pages. Run after any
edit to index, pricing, download or comparison:

    python3 scripts/site_es.py
"""
import re, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1] / 'site'
PAGES = ['index.html', 'pricing.html', 'download.html', 'comparison.html']
ENGLISH_ONLY = ['privacy.html', 'terms.html', 'success.html', '404.html']
ASSETS = ['favicon.ico', 'favicon.svg', 'icon.png', 'apple-touch-icon.png', 'demo-v7.mp4', 'demo-poster-v7.jpg']


def must(s, old, new, count=1):
    assert s.count(old) == count, (old[:70], s.count(old), count)
    return s.replace(old, new)


def maybe(s, old, new):
    assert s.count(old) <= 1, (old[:70], s.count(old))
    return s.replace(old, new)


# ── the flags ───────────────────────────────────────────────────────────────
_h = 40 / 13
US = ('<svg viewBox="0 0 60 40" aria-hidden="true"><rect width="60" height="40" fill="#fff"/>'
      + ''.join(f'<rect y="{i*_h:.2f}" width="60" height="{_h+0.05:.2f}" fill="#B22234"/>' for i in range(0, 13, 2))
      + f'<rect width="24" height="{7*_h:.2f}" fill="#3C3B6E"/>'
      + ''.join(f'<circle cx="{2.4+c*4.8:.1f}" cy="{2.15+r*4.3:.2f}" r="1" fill="#fff"/>' for r in range(5) for c in range(5))
      + '</svg>')
ES = ('<svg viewBox="0 0 60 40" aria-hidden="true"><rect width="60" height="40" fill="#AA151B"/>'
      '<rect y="10" width="60" height="20" fill="#F1BF00"/></svg>')


def lang_switch(cur, en_href, es_href):
    en_on = ' is-on" aria-current="page"' if cur == 'en' else '"'
    es_on = ' is-on" aria-current="page"' if cur == 'es' else '"'
    label = 'Language' if cur == 'en' else 'Idioma'
    return (f'<div class="lang" role="group" aria-label="{label}">\n'
            f'        <a class="lang-opt{en_on} href="{en_href}" hreflang="en" lang="en" title="English" aria-label="English">{US}</a>\n'
            f'        <a class="lang-opt{es_on} href="{es_href}" hreflang="es" lang="es" title="Español" aria-label="Español">{ES}</a>\n'
            f'      </div>\n'
            f'      <button type="button" class="mode-toggle"')


LANG_CSS = '''  /* language: two flags, the current one lit with a hairline, both kept on phones */
  .lang { display: inline-flex; align-items: center; gap: 6px; }
  .lang-opt {
    display: inline-flex; width: 24px; height: 17px; border-radius: 4px; overflow: hidden;
    opacity: .45; border: 1px solid var(--line-2); transition: opacity .15s, box-shadow .15s;
  }
  .lang-opt svg { display: block; width: 100%; height: 100%; }
  .lang-opt:hover { opacity: .85; }
  .lang-opt.is-on { opacity: 1; border-color: var(--jade); }
  .lang-opt:focus-visible { outline: 2px solid var(--jade); outline-offset: 2px; }
  /* buttons, chips and the platform picker share the switch's corner radius */'''

EN_BOOT = '''    document.documentElement.dataset.theme = t;
    // Language: a flag click is remembered on every page; a Spanish browser's first visit to the
    // home page lands on the Spanish page. 'here' is '' for the home page, else 'pricing' etc.
    try {
      var lang = localStorage.getItem('lang');
      var pref = (navigator.languages && navigator.languages[0]) || navigator.language || '';
      var here = location.pathname.replace(/^.*\\//, '').replace(/\\.html$/, '').replace(/^index$/, '');
      if (!/^#thanks/.test(location.hash) && (lang === 'es' || (!lang && !here && /^es\\b/i.test(pref))))
        location.replace('es/' + (here ? here + '.html' : '') + location.hash);
    } catch (e) {}
'''
ES_BOOT = '''    document.documentElement.dataset.theme = t;
    // Idioma: quien eligió inglés con la bandera vuelve a la página en inglés.
    try {
      var here = location.pathname.replace(/^.*\\//, '').replace(/\\.html$/, '').replace(/^index$/, '');
      if (localStorage.getItem('lang') === 'en') location.replace('../' + (here ? here + '.html' : '') + location.hash);
    } catch (e) {}
'''

LANG_JS = '''    paint();
    // A flag click is a language choice; the home page stops auto-detecting after it.
    document.querySelectorAll('.lang-opt').forEach(function (a) {
      a.addEventListener('click', function () { try { localStorage.setItem('lang', a.getAttribute('hreflang')); } catch (e) {} });
    });
  })();
</script>'''

# ── copy shared by every page: nav, switch labels, footer, theme script ─────
COMMON = [
    ('<a href="index.html#features">Features</a>', '<a href="index.html#features">Funciones</a>'),
    ('<a href="#features">Features</a>', '<a href="#features">Funciones</a>'),
    ('<a href="index.html#platforms">Platforms</a>', '<a href="index.html#platforms">Plataformas</a>'),
    ('<a href="#platforms">Platforms</a>', '<a href="#platforms">Plataformas</a>'),
    ('<a href="pricing.html">Pricing</a>', '<a href="pricing.html">Precios</a>'),
    ('<a class="nav-cta" href="download.html">Download</a>', '<a class="nav-cta" href="download.html">Descargar</a>'),
    ('aria-label="Light theme" title="Switch to dark"', 'aria-label="Tema claro" title="Cambiar a oscuro"'),
    ("sw.setAttribute('aria-label', dark ? 'Dark theme' : 'Light theme');", "sw.setAttribute('aria-label', dark ? 'Tema oscuro' : 'Tema claro');"),
    ("sw.title = dark ? 'Switch to light' : 'Switch to dark';", "sw.title = dark ? 'Cambiar a claro' : 'Cambiar a oscuro';"),
    ('<span>© 2026 Smartloop Inc. All rights reserved.</span>', '<span>© 2026 Smartloop Inc. Todos los derechos reservados.</span>'),
    ('<span>Made in San Francisco, CA</span>', '<span>Hecho en San Francisco, CA</span>'),
    ('<a href="comparison.html">compare</a>', '<a href="comparison.html">comparar</a>'),
    ('<a href="privacy.html">privacy</a>', '<a href="privacy.html">privacidad</a>'),
    ('<a href="terms.html">terms</a>', '<a href="terms.html">términos</a>'),
    ('aria-label="ScreenForge on X" title="@ScreenForgeHQ on X"', 'aria-label="ScreenForge en X" title="@ScreenForgeHQ en X"'),
]

# ── index ───────────────────────────────────────────────────────────────────
INDEX_TITLE = 'ScreenForge — Screencasts pro en minutos. Pídelo y la IA lo edita.'
INDEX_DESC = 'Graba tu pantalla y tu cámara, o abre cualquier vídeo, y pide a un agente de IA que lo edite: cortes, zooms, subtítulos, música y voz en off. Gratis y de código abierto para Mac, Windows y Linux.'
INDEX_JSONLD = '''<script type="application/ld+json">
{
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "SoftwareApplication",
      "name": "ScreenForge",
      "url": "https://screenforge.studio/es/",
      "inLanguage": "es",
      "image": "https://screenforge.studio/og.png?v=6",
      "alternateName": "ScreenForge editor de pantalla con IA",
      "applicationSubCategory": "Grabador de pantalla y editor de vídeo",
      "description": "Grabador de pantalla y editor de vídeo con IA para tutoriales, demos de producto, vídeos a cámara y screencasts. Graba tu pantalla, tu webcam y, en Mac, un iPhone o iPad. Un agente de IA edita a partir de peticiones en lenguaje natural: cortes, zooms, velocidad, música y fondos. La narración con voz de IA y los subtítulos se generan en tu ordenador. Edita también tus propios vídeos MP4 y MOV. Una alternativa de código abierto a Screen Studio y CapCut.",
      "keywords": "editor de vídeo con IA, grabador de pantalla con IA, software de screencast, alternativa a Screen Studio, alternativa a CapCut, voz en off con IA, subtítulos automáticos",
      "featureList": ["Agente de edición con IA", "Narración con voz de IA, en el dispositivo o con voces de estudio de ElevenLabs", "Subtítulos automáticos con Whisper en el dispositivo", "Zooms automáticos", "Grabación de cámara con seguimiento facial", "Etalonaje de color", "Grabación de iPhone y iPad (macOS)", "Marcos de móvil y tablet", "Grabación de voz en off", "Cursor suave", "Fondos", "Pista de voz separada", "Máscaras de difuminado y resaltado", "Recorte", "Atajos de teclado en pantalla", "Edita cualquier vídeo MP4, MOV o M4V"],
      "applicationCategory": "MultimediaApplication",
      "operatingSystem": "macOS 14 o posterior, Windows 10 o posterior, Linux",
      "license": "https://www.apache.org/licenses/LICENSE-2.0",
      "offers": [
        {"@type": "Offer", "name": "Gratis", "price": "0", "priceCurrency": "USD"},
        {"@type": "Offer", "name": "Pro, con el agente de IA", "price": "20", "priceCurrency": "USD", "priceSpecification": {"@type": "UnitPriceSpecification", "price": "20", "priceCurrency": "USD", "billingDuration": "P1M"}},
        {"@type": "Offer", "name": "Licencia Creator", "price": "49", "priceCurrency": "USD"}
      ],
      "publisher": {"@type": "Organization", "name": "Smartloop Inc.", "url": "https://screenforge.studio/", "logo": "https://screenforge.studio/icon.png"},
      "sameAs": ["https://x.com/ScreenForgeHQ"]
    },
    {
      "@type": "VideoObject",
      "name": "El agente de IA de ScreenForge edita un screencast a partir de una sola petición",
      "inLanguage": "en",
      "description": "Con una sola petición, el agente recorta la grabación a menos de un minuto, acelera las esperas, añade música de fondo y una foto de montañas de Unsplash como fondo, y narra el recorrido con una voz de IA y subtítulos.",
      "thumbnailUrl": "https://screenforge.studio/demo-poster-v7.jpg",
      "contentUrl": "https://screenforge.studio/demo-v7.mp4",
      "uploadDate": "2026-10-24",
      "duration": "PT30S"
    }
  ]
}
</script>'''

INDEX = [
    ('Pro demos in minutes.<br><em>Just ask. It edits.', 'Demos pro en minutos.<br><em>Pídelo. La IA lo edita.'),
    ('aria-label="Things you can ask the agent"', 'aria-label="Cosas que puedes pedir al agente"'),
    ('<li>cut the ums and the long pauses</li>', '<li>quita los “eh” y las pausas largas</li>'),
    ('<li>zoom on my clicks</li>', '<li>haz zoom en mis clics</li>'),
    ('<li>get this under a minute</li>', '<li>déjalo en menos de un minuto</li>'),
    ('<li>add calm music and a mountain photo</li>', '<li>añade música tranquila y una foto de montañas</li>'),
    ('''      A screen recorder and AI video editor in one. Record your <span class="os-name">Mac</span> screen and camera, or drop in any video, then ask for the edit:
      zooms, a smooth cursor, backgrounds, captions, music and voice-over, on a timeline you can still edit by hand.''',
     '''      Grabador de pantalla y editor de vídeo con IA en una sola app. Graba la pantalla y la cámara de tu <span class="os-name">Mac</span>, o abre cualquier vídeo, y pide la edición:
      zooms, cursor suave, fondos, subtítulos, música y voz en off, en una línea de tiempo que también puedes editar a mano.'''),
    ('<span class="os-label">Download for macOS</span>', '<span class="os-label">Descargar para macOS</span>', 2),
    ('<span class="note os-note">Free to record and edit · <a href="pricing.html">Pro US$20/month</a> adds the AI agent · open source · Apple silicon, macOS 14 or later · also on <a href="download.html#windows">Windows</a> and <a href="download.html#linux">Linux</a></span>',
     '<span class="note os-note">Grabar y editar es gratis · <a href="pricing.html">Pro, 20 US$/mes</a> añade el agente de IA · código abierto · Apple silicon, macOS 14 o posterior · también en <a href="download.html#windows">Windows</a> y <a href="download.html#linux">Linux</a></span>'),
    ('aria-label="The ScreenForge agent editing a recording from one prompt: it trims the video under a minute, speeds up the waits, adds background music, an Unsplash photo of mountains as the wallpaper, and AI narration with captions"',
     'aria-label="El agente de ScreenForge edita una grabación con una sola petición: recorta el vídeo a menos de un minuto, acelera las esperas, añade música de fondo, una foto de montañas de Unsplash como fondo y narración de IA con subtítulos"'),
    ('<li><div><b>Record</b><span>Your screen, your camera or an iPhone. Talk it through.</span></div></li>', '<li><div><b>Graba</b><span>Tu pantalla, tu cámara o un iPhone. Cuéntalo mientras grabas.</span></div></li>'),
    ('<li><div><b>Edit</b><span>Ask the agent: “cut the ums, zoom on my clicks”. Or edit by hand.</span></div></li>', '<li><div><b>Edita</b><span>Pide al agente: “quita los eh, haz zoom en mis clics”. O edita a mano.</span></div></li>'),
    ('<li><div><b>Export</b><span>MP4 or GIF, up to 4K, ready for YouTube.</span></div></li>', '<li><div><b>Exporta</b><span>MP4 o GIF, hasta 4K, listo para YouTube.</span></div></li>'),
    ('<div class="sec-label">What it does</div>', '<div class="sec-label">Qué hace</div>'),
    ('<h2>Record, edit, export.<br><em>One native app.</em></h2>', '<h2>Graba, edita, exporta.<br><em>Una app nativa.</em></h2>'),
    ('<p class="sec-sub">A recorder and a timeline editor in one window. No Electron, no web view; the compositor runs on the GPU and the UI is drawn natively.</p>',
     '<p class="sec-sub">Grabador y editor de línea de tiempo en una sola ventana. Sin Electron ni vista web; el compositor corre en la GPU y la interfaz se dibuja de forma nativa.</p>'),
    ('<div class="k">AI editing agent <span class="tag">new</span></div>', '<div class="k">Agente de edición con IA <span class="tag">nuevo</span></div>'),
    ('<h3>Edit by asking</h3>', '<h3>Edita pidiéndolo</h3>'),
    ('<p>Say what you want and watch the timeline change as the agent works. It reads the transcript, your clicks and the video itself, then cuts, zooms, speeds up, adds music or narration. Auto edit tightens the whole video; one <code>⌘Z</code> undoes a whole answer.</p>',
     '<p>Di lo que quieres y mira cómo cambia la línea de tiempo mientras el agente trabaja. Lee la transcripción, tus clics y el propio vídeo, y luego corta, hace zoom, acelera, añade música o narración. La edición automática ajusta todo el vídeo; un solo <code>⌘Z</code> deshace una respuesta completa.</p>'),
    ('<div class="q">cut the ums and the long pauses, zoom on my clicks, add calm music</div>', '<div class="q">quita los eh y las pausas largas, haz zoom en mis clics, añade música tranquila</div>'),
    ('<span class="ok">14 cuts</span><span class="ok">6 zooms</span><span class="ok">music: Calm Focus</span><span>0:58 from 1:41</span><span>one undo</span>',
     '<span class="ok">14 cortes</span><span class="ok">6 zooms</span><span class="ok">música: Calm Focus</span><span>0:58 desde 1:41</span><span>un deshacer</span>'),
    ('<h3>Automatic zooms</h3>', '<h3>Zooms automáticos</h3>'),
    ('<p>Zooms open on your clicks and follow the cursor as you work, easing in and out. Adjust or remove any of them on the timeline.</p>',
     '<p>Los zooms se abren en tus clics y siguen al cursor mientras trabajas, con entradas y salidas suaves. Ajusta o elimina cualquiera en la línea de tiempo.</p>'),
    ('<div class="k">Camera</div>', '<div class="k">Cámara</div>'),
    ('<h3>You on camera</h3>', '<h3>Tú en cámara</h3>'),
    ('<p>Overlay, split or full screen, switched along the timeline. The camera follows your face, and Resolve-style colour grading with LUTs makes it look right.</p>',
     '<p>Superpuesta, dividida o a pantalla completa, cambiando a lo largo de la línea de tiempo. La cámara sigue tu cara, y el etalonaje de color al estilo Resolve con LUTs la hace lucir bien.</p>'),
    ('<div class="k">Captions</div>', '<div class="k">Subtítulos</div>'),
    ('<h3>Captions from your voice</h3>', '<h3>Subtítulos a partir de tu voz</h3>'),
    ('<p>Transcribed on your computer with Whisper, nothing uploaded. Line by line or word by word, burned into the export or saved as SRT and WebVTT.</p>',
     '<p>Transcritos en tu ordenador con Whisper, sin subir nada. Línea a línea o palabra a palabra, incrustados en la exportación o guardados como SRT y WebVTT.</p>'),
    ('<div class="k">Voice-over</div>', '<div class="k">Voz en off</div>'),
    ('<h3>Narrate after you record</h3>', '<h3>Narra después de grabar</h3>'),
    ('<p>Press <code>R</code> and talk over the video, or type a script and an AI voice reads it: six voices on your computer, eight studio voices online. Each take sits on its own lane.</p>',
     '<p>Pulsa <code>R</code> y habla sobre el vídeo, o escribe un guion y una voz de IA lo lee: seis voces en tu ordenador, ocho voces de estudio en línea. Cada toma va en su propia pista.</p>'),
    ('<div class="k">Any video</div>', '<div class="k">Cualquier vídeo</div>'),
    ('<h3>Edit any MP4 or MOV</h3>', '<h3>Edita cualquier MP4 o MOV</h3>'),
    ('<p>Drop a video on ScreenForge to start a project, or on an open project to add it to the timeline. Screen Studio projects open too.</p>',
     '<p>Suelta un vídeo sobre ScreenForge para crear un proyecto, o sobre un proyecto abierto para añadirlo a la línea de tiempo. Los proyectos de Screen Studio también se abren.</p>'),
    ('<div class="k">Privacy and framing</div>', '<div class="k">Privacidad y encuadre</div>'),
    ('<h3>Blur, highlight, crop</h3>', '<h3>Difumina, resalta, recorta</h3>'),
    ('<p>Blur passwords and private details or dim everything but the part that matters, timed on the timeline. Crop to the part of the screen you need; zooms and cursor follow.</p>',
     '<p>Difumina contraseñas y datos privados o atenúa todo salvo la parte que importa, con tiempos en la línea de tiempo. Recorta a la zona de pantalla que necesitas; los zooms y el cursor la siguen.</p>'),
    ('<div class="k">Native and fast</div>', '<div class="k">Nativo y rápido</div>'),
    ('<h3>GPU rendering, native capture</h3>', '<h3>Render en GPU, captura nativa</h3>'),
    ('<p>ScreenCaptureKit, Desktop Duplication or PipeWire capture; a wgpu compositor renders the timeline. Export MP4, HEVC, ProRes, GIF or WebM with mixed audio.</p>',
     '<p>Captura con ScreenCaptureKit, Desktop Duplication o PipeWire; un compositor wgpu renderiza la línea de tiempo. Exporta MP4, HEVC, ProRes, GIF o WebM con audio mezclado.</p>'),
    ('<div class="k">Devices and keys</div>', '<div class="k">Dispositivos y teclas</div>'),
    ('<h3>iPhone frames, shortcuts on screen</h3>', '<h3>Marcos de iPhone, atajos en pantalla</h3>'),
    ('<p>Record an iPhone or iPad over a cable and show it in a device frame. Shortcuts you press appear as key caps, like <code>⇧ ⌘ S</code>, so viewers can follow along.</p>',
     '<p>Graba un iPhone o iPad por cable y muéstralo en un marco de dispositivo. Los atajos que pulsas aparecen como teclas, como <code>⇧ ⌘ S</code>, para que el público pueda seguirte.</p>'),
    ('<div class="k">Looks and sound</div>', '<div class="k">Imagen y sonido</div>'),
    ('<h3>Backgrounds, frames and music</h3>', '<h3>Fondos, marcos y música</h3>'),
    ("<p>Set the recording on a wallpaper, your desktop's own, a gradient or an Unsplash photo, with shadow and inset. Put a phone video in a device frame. Pick a track from the built-in music library, lo-fi to cinematic, or drop in your own.</p>",
     '<p>Coloca la grabación sobre un fondo: el de tu escritorio, un degradado o una foto de Unsplash, con sombra y margen. Pon un vídeo de móvil en un marco de dispositivo. Elige una pista de la biblioteca de música integrada, de lo-fi a cinematográfica, o añade la tuya.</p>'),
    ('<div class="sec-label">Platforms</div>', '<div class="sec-label">Plataformas</div>'),
    ('<h2>One app. <em>Every desktop.</em></h2>', '<h2>Una app. <em>Todos los escritorios.</em></h2>'),
    ('<p class="sec-sub">The same recorder and editor on all three, with projects that open anywhere. Recording and editing are free on each.</p>',
     '<p class="sec-sub">El mismo grabador y editor en las tres, con proyectos que se abren en cualquiera. Grabar y editar es gratis en todas.</p>'),
    ('<div class="req">Apple silicon · macOS 14 or later · signed and notarized</div>', '<div class="req">Apple silicon · macOS 14 o posterior · firmada y notarizada</div>'),
    ('<p>Screen, window or area with ScreenCaptureKit, webcam, system audio, and iPhone or iPad over a cable. Also on Homebrew.</p>',
     '<p>Pantalla, ventana o área con ScreenCaptureKit, webcam, audio del sistema, e iPhone o iPad por cable. También en Homebrew.</p>'),
    ('<a class="alt" href="download.html#mac">Download for macOS</a>', '<a class="alt" href="download.html#mac">Descargar para macOS</a>'),
    ('<div class="req">x86_64 · Windows 10 or later · installer or portable zip</div>', '<div class="req">x86_64 · Windows 10 o posterior · instalador o zip portable</div>'),
    ('<p>Desktop Duplication capture with cursor, clicks, keystrokes and system audio, just like the Mac build. Signed by Smartloop Inc.</p>',
     '<p>Captura con Desktop Duplication con cursor, clics, pulsaciones y audio del sistema, igual que en Mac. Firmada por Smartloop Inc.</p>'),
    ('<a class="alt" href="download.html#windows">Download for Windows</a>', '<a class="alt" href="download.html#windows">Descargar para Windows</a>'),
    ('<div class="req">x86_64 · X11 and Wayland · .deb or .tar.gz</div>', '<div class="req">x86_64 · X11 y Wayland · .deb o .tar.gz</div>'),
    ('<p>Native PipeWire capture on Wayland, full input capture on X11, ffmpeg included. Also on Homebrew.</p>',
     '<p>Captura nativa con PipeWire en Wayland, captura completa de entrada en X11, ffmpeg incluido. También en Homebrew.</p>'),
    ('<a class="alt" href="download.html#linux">Download for Linux</a>', '<a class="alt" href="download.html#linux">Descargar para Linux</a>'),
    ('<div class="trust" aria-label="Good to know">', '<div class="trust" aria-label="Conviene saber">'),
    ('<span>Open source, Apache-2.0</span><span>Recordings never leave your computer</span><span>No analytics in the app</span><span>Captions and voices run on device</span>',
     '<span>Código abierto, Apache-2.0</span><span>Las grabaciones nunca salen de tu ordenador</span><span>Sin analíticas en la app</span><span>Subtítulos y voces en tu dispositivo</span>'),
    ('<div class="sec-label">Pricing</div>', '<div class="sec-label">Precios</div>'),
    ('<h2>Pricing that fits<br><em>how you work.</em></h2>', '<h2>Precios que se ajustan<br><em>a tu forma de trabajar.</em></h2>'),
    ('<p class="sec-sub">Recording and editing are free and open source under Apache-2.0. Exporting needs a license: Pro adds the agent with 1,000 credits a month and unlimited export; Creator is export alone, US$49 once. Export keys are verified offline.</p>',
     '<p class="sec-sub">Grabar y editar es gratis y de código abierto bajo Apache-2.0. Exportar requiere licencia: Pro añade el agente con 1.000 créditos al mes y exportación ilimitada; Creator es solo exportación, 49 US$ una vez. Las claves de exportación se verifican sin conexión.</p>'),
    ('<div class="row"><b>Free</b><span>Record, open and edit projects. <i>Forever, no license needed.</i></span></div>', '<div class="row"><b>Gratis</b><span>Graba, abre y edita proyectos. <i>Para siempre, sin licencia.</i></span></div>'),
    ('<div class="row"><b>Source</b><span>Apache-2.0 on GitHub. <i>The license funds the signed, ready-made build and its upkeep.</i></span></div>', '<div class="row"><b>Código</b><span>Apache-2.0 en GitHub. <i>La licencia financia la versión firmada y lista para usar, y su mantenimiento.</i></span></div>'),
    ('<div class="row"><b>Offline</b><span>Export keys are checked on your machine. <i>The AI agent runs on Claude, through our server.</i></span></div>', '<div class="row"><b>Sin conexión</b><span>Las claves de exportación se comprueban en tu equipo. <i>El agente de IA funciona con Claude, a través de nuestro servidor.</i></span></div>'),
    ('<div class="row"><b>Refunds</b><span>Within 24 hours. <i>Pro: before using credits.</i></span></div>', '<div class="row"><b>Reembolsos</b><span>En 24 horas. <i>Pro: antes de usar créditos.</i></span></div>'),
    ('<div class="fig">US$20<small>/month</small></div>', '<div class="fig">20 US$<small>/mes</small></div>'),
    ('<div class="per">cancel anytime · credits refill every month</div>', '<div class="per">cancela cuando quieras · los créditos se renuevan cada mes</div>'),
    ('<li>The AI editing agent: ask for edits, or Auto edit the whole video</li>', '<li>El agente de edición con IA: pide cambios o deja que edite todo el vídeo automáticamente</li>'),
    ('<li>1,000 agent credits a month; top-up packs never expire</li>', '<li>1.000 créditos de agente al mes; los paquetes extra no caducan</li>'),
    ('<li>Unlimited export to MP4, ProRes, GIF and WebM</li>', '<li>Exportación ilimitada a MP4, ProRes, GIF y WebM</li>'),
    ('<li>Every future update included</li>', '<li>Todas las actualizaciones futuras incluidas</li>'),
    ('<a class="buy" href="pricing.html">Get Pro</a>', '<a class="buy" href="pricing.html">Conseguir Pro</a>'),
    ('<p class="foot">Only need export? Creator is US$49 once. <a href="pricing.html">See all plans</a></p>', '<p class="foot">¿Solo necesitas exportar? Creator cuesta 49 US$ una vez. <a href="pricing.html">Ver todos los planes</a></p>'),
    ('<div class="sec-label">Get started</div>', '<div class="sec-label">Empieza</div>'),
    ('<h2>Record your first clip in <em>a minute.</em></h2>', '<h2>Graba tu primer clip en <em>un minuto.</em></h2>'),
    ("l.textContent = 'Download for ' + name;", "l.textContent = 'Descargar para ' + name;"),
    ('''        ? 'Free to record and edit · <a href="pricing.html">Pro US$20/month</a> adds the AI agent · open source · x86_64, .deb or .tar.gz · also on <a href="download.html#mac">macOS</a> and <a href="download.html#windows">Windows</a>'
        : 'Free to record and edit · <a href="pricing.html">Pro US$20/month</a> adds the AI agent · open source · x86_64, Windows 10 or later · also on <a href="download.html#mac">macOS</a> and <a href="download.html#linux">Linux</a>';''',
     '''        ? 'Grabar y editar es gratis · <a href="pricing.html">Pro, 20 US$/mes</a> añade el agente de IA · código abierto · x86_64, .deb o .tar.gz · también en <a href="download.html#mac">macOS</a> y <a href="download.html#windows">Windows</a>'
        : 'Grabar y editar es gratis · <a href="pricing.html">Pro, 20 US$/mes</a> añade el agente de IA · código abierto · x86_64, Windows 10 o posterior · también en <a href="download.html#mac">macOS</a> y <a href="download.html#linux">Linux</a>';'''),
]

# ── pricing ─────────────────────────────────────────────────────────────────
PRICING_TITLE = 'Precios — ScreenForge: gratis, Pro con el agente de IA o una licencia de pago único'
PRICING_DESC = 'ScreenForge es gratis para grabar y editar. Pro cuesta 20 US$/mes con el agente de edición con IA, 1.000 créditos al mes y exportación ilimitada; la licencia Creator cuesta 49 US$ una vez. Reembolso en 24 horas de compras sin usar.'
PRICING = [
    ('<div class="eyebrow">Pricing</div>', '<div class="eyebrow">Precios</div>'),
    ('<h1>Pricing that fits <em>how you work.</em></h1>', '<h1>Precios que se ajustan <em>a tu forma de trabajar.</em></h1>'),
    ('<p class="stamp">free to record and edit · Pro US$20/month with the AI agent · Creator US$49 once · 24-hour refund (Pro: before using credits)</p>',
     '<p class="stamp">grabar y editar es gratis · Pro 20 US$/mes con el agente de IA · Creator 49 US$ una vez · reembolso en 24 horas (Pro: antes de usar créditos)</p>'),
    ('<div class="name">Free</div>', '<div class="name">Gratis</div>'),
    ('<div class="fig">US$0<small>forever</small></div>', '<div class="fig">0 US$<small>para siempre</small></div>'),
    ('<div class="per">no account · no key · no card</div>', '<div class="per">sin cuenta · sin clave · sin tarjeta</div>'),
    ('<li>Record a display, a window or an area</li>', '<li>Graba una pantalla, una ventana o un área</li>'),
    ('<li>Automatic zooms, captions and the independent audio track</li>', '<li>Zooms automáticos, subtítulos y la pista de audio independiente</li>'),
    ('<li>Apache-2.0 source on <a href="https://github.com/mehfuzh/screenforge">GitHub</a></li>', '<li>Código Apache-2.0 en <a href="https://github.com/mehfuzh/screenforge">GitHub</a></li>'),
    ('<span class="os-label">Download for macOS</span>', '<span class="os-label">Descargar para macOS</span>'),
    ('<p class="foot">Everything on the timeline works without paying. Exporting from the official build is where the key comes in.</p>',
     '<p class="foot">Todo lo de la línea de tiempo funciona sin pagar. La clave entra en juego al exportar desde la versión oficial.</p>'),
    ('<div class="name">Pro <span class="tag">AI agent</span></div>', '<div class="name">Pro <span class="tag">agente de IA</span></div>'),
    ('<div class="fig">US$20<small>/month</small></div>', '<div class="fig">20 US$<small>/mes</small></div>'),
    ('<div class="per">cancel anytime · credits refill every month</div>', '<div class="per">cancela cuando quieras · los créditos se renuevan cada mes</div>'),
    ('<button class="buy" type="submit">Get Pro</button>', '<button class="buy" type="submit">Conseguir Pro</button>'),
    ('<li>The AI editing agent: ask for edits in plain words, or Auto edit the whole video</li>', '<li>El agente de edición con IA: pide cambios con tus palabras o deja que edite todo el vídeo automáticamente</li>'),
    ('<li>1,000 credits every month, for the agent and studio-quality ElevenLabs voices</li>', '<li>1.000 créditos cada mes, para el agente y las voces de estudio de ElevenLabs</li>'),
    ('<li>Top up anytime: credit packs never expire</li>', '<li>Recarga cuando quieras: los paquetes de créditos no caducan</li>'),
    ('<li>Unlimited export, like Creator</li>', '<li>Exportación ilimitada, como Creator</li>'),
    ('<li>Email support from a person, usually the same day</li>', '<li>Soporte por correo de una persona, normalmente el mismo día</li>', 2),
    ('<p class="foot">Stripe takes the payment and asks for your email there. The key arrives by email; activate it in ScreenForge with the same email, and the credits show in the agent panel.</p>',
     '<p class="foot">Stripe gestiona el pago y te pide allí tu correo. La clave llega por correo; actívala en ScreenForge con el mismo correo y los créditos aparecen en el panel del agente.</p>'),
    ('<div class="fig">US$49<small>once</small><span class="tag">lifetime</span></div>', '<div class="fig">49 US$<small>una vez</small><span class="tag">de por vida</span></div>'),
    ('<div class="per">one payment · the key never expires</div>', '<div class="per">un solo pago · la clave nunca caduca</div>'),
    ('<button class="buy" type="submit">Buy lifetime</button>', '<button class="buy" type="submit">Comprar de por vida</button>'),
    ('<li>Unlimited export to MP4 (H.264 / HEVC), ProRes, GIF and WebM</li>', '<li>Exportación ilimitada a MP4 (H.264 / HEVC), ProRes, GIF y WebM</li>'),
    ('<li>Verified offline; works without a network</li>', '<li>Verificada sin conexión; funciona sin red</li>'),
    ('<li>Every future update included</li>', '<li>Todas las actualizaciones futuras incluidas</li>'),
    ('<li>One key per email, active on one computer at a time; move it whenever you switch</li>', '<li>Una clave por correo, activa en un ordenador a la vez; muévela cuando cambies</li>'),
    ('<p class="foot">Stripe takes the payment and asks for your email there. The key arrives by email within a minute; in ScreenForge open Export, enter the same email and press Activate, then type the 6-digit code we email you to confirm the computer.</p>',
     '<p class="foot">Stripe gestiona el pago y te pide allí tu correo. La clave llega por correo en un minuto; en ScreenForge abre Exportar, escribe el mismo correo y pulsa Activar, luego introduce el código de 6 dígitos que te enviamos para confirmar el ordenador.</p>'),
    ('<div class="name">Agent credits</div>', '<div class="name">Créditos del agente</div>'),
    ('<p>The AI agent runs on Claude. Each request uses credits for what it costs: one credit is about one cent of Claude usage, and a typical edit takes a few. The same credits pay for narration with ElevenLabs voices: about 8 credits per 1,000 characters, around 7 for a minute of narration (the on-device voices are free). Pro includes 1,000 a month; when they run out, add a pack. Packs never expire and are used after the monthly credits. The agent is part of Pro, so packs top up a Pro subscription.</p>',
     '<p>El agente de IA funciona con Claude. Cada petición consume créditos según lo que cuesta: un crédito equivale a un centavo de uso de Claude, y una edición típica consume unos pocos. Los mismos créditos pagan la narración con voces de ElevenLabs: unos 8 créditos por cada 1.000 caracteres, alrededor de 7 por un minuto de narración (las voces del dispositivo son gratis). Pro incluye 1.000 al mes; cuando se acaban, añade un paquete. Los paquetes no caducan y se usan después de los créditos mensuales. El agente forma parte de Pro, así que los paquetes recargan una suscripción Pro.</p>'),
    ('<p>Buy packs in ScreenForge, from the credits menu in the agent panel.</p>', '<p>Compra paquetes en ScreenForge, desde el menú de créditos del panel del agente.</p>'),
    ('<li>500 credits: US$10</li>', '<li>500 créditos: 10 US$</li>'),
    ('<li>1,250 credits: US$25</li>', '<li>1.250 créditos: 25 US$</li>'),
    ('<li>2,500 credits: US$50</li>', '<li>2.500 créditos: 50 US$</li>'),
    ('<li>Monthly credits refill on your billing date and do not carry over</li>', '<li>Los créditos mensuales se renuevan en tu fecha de facturación y no se acumulan</li>'),
    ('<div class="name">Studios &amp; teams</div>', '<div class="name">Estudios y equipos</div>'),
    ("<p>Design studios, agencies and product teams that record a lot get one invoice for every seat and keys issued in bulk to each person's email. Existing screen recording projects open as they are, so nobody has to redo their work.</p>",
     '<p>Los estudios de diseño, agencias y equipos de producto que graban mucho reciben una sola factura por todos los puestos y claves emitidas en bloque al correo de cada persona. Los proyectos de grabación existentes se abren tal cual, así que nadie tiene que rehacer su trabajo.</p>'),
    ('<p>To ask about a team license, reach out to <a href="mailto:license@screenforge.studio">license@screenforge.studio</a> with your studio name, how many people record and what you use today. A person replies, usually the same day.</p>',
     '<p>Para preguntar por una licencia de equipo, escribe a <a href="mailto:license@screenforge.studio">license@screenforge.studio</a> con el nombre de tu estudio, cuántas personas graban y qué usáis hoy. Responde una persona, normalmente el mismo día.</p>'),
    ('<li>Volume pricing for 5 seats and up: from US$39 a seat</li>', '<li>Precio por volumen a partir de 5 puestos: desde 39 US$ por puesto</li>'),
    ('<li>One invoice, card or bank transfer</li>', '<li>Una sola factura, con tarjeta o transferencia</li>'),
    ('<li>Keys for every teammate, re-issued when people change</li>', '<li>Claves para cada miembro del equipo, reemitidas cuando cambia la gente</li>'),
    ('<li>Shared presets so every video matches the brand</li>', '<li>Ajustes compartidos para que cada vídeo siga la marca</li>'),
    ('<li>Priority email support and a say in the roadmap</li>', '<li>Soporte prioritario por correo y voz en la hoja de ruta</li>'),
    ('<h2>Questions</h2>', '<h2>Preguntas</h2>'),
    ('<h3>What does the AI agent cost?</h3>', '<h3>¿Cuánto cuesta el agente de IA?</h3>'),
    ('<p>What Claude charges for it, in credits: a short edit takes a few credits, an Auto edit of a long video more. The agent uses Claude Sonnet 5.5 unless you pick another model: Opus 5.5 uses credits twice as fast, Haiku 5.5 about 20 times slower. Pro includes 1,000 credits a month, enough for daily editing; packs top it up and never expire. The agent panel shows your balance and what each conversation used.</p>',
     '<p>Lo que Claude cobra por ello, en créditos: una edición corta consume unos pocos créditos, una edición automática de un vídeo largo, más. El agente usa Claude Sonnet 5.5 salvo que elijas otro modelo: Opus 5.5 gasta créditos el doble de rápido, Haiku 5.5 unas 20 veces más despacio. Pro incluye 1.000 créditos al mes, suficientes para editar a diario; los paquetes lo recargan y no caducan. El panel del agente muestra tu saldo y lo que ha consumido cada conversación.</p>'),
    ('<h3>What do the ElevenLabs voices cost?</h3>', '<h3>¿Cuánto cuestan las voces de ElevenLabs?</h3>'),
    ("<p>They come with Pro and use the same credits as the agent: about 8 credits per 1,000 characters of script, so a minute of narration is around 7 credits and 1,000 credits read about two hours. Hearing a voice's preview again is free. The six voices that run on your computer stay free for everyone.</p>",
     '<p>Vienen con Pro y usan los mismos créditos que el agente: unos 8 créditos por cada 1.000 caracteres de guion, así que un minuto de narración son unos 7 créditos y 1.000 créditos leen unas dos horas. Volver a escuchar la muestra de una voz es gratis. Las seis voces que funcionan en tu ordenador siguen siendo gratis para todos.</p>'),
    ('<h3>How is the key checked?</h3>', '<h3>¿Cómo se comprueba la clave?</h3>'),
    ("<p>On your computer. The app verifies the key's signature locally, so exporting works offline. When it is online it briefly checks in with the license server, so a license moved to another computer stops working on the old one.</p>",
     '<p>En tu ordenador. La app verifica la firma de la clave en local, así que exportar funciona sin conexión. Cuando hay red, consulta brevemente el servidor de licencias, de modo que una licencia movida a otro ordenador deja de funcionar en el anterior.</p>'),
    ('<h3>How many computers can I use it on?</h3>', '<h3>¿En cuántos ordenadores puedo usarla?</h3>'),
    ('<p>One at a time. Activating on another computer moves the license there with a code emailed to the address you paid with, so nobody can use your license just by knowing your email, and you never need to contact us to switch Macs. The previous computer stops exporting the next time it is online.</p>',
     '<p>En uno a la vez. Activarla en otro ordenador mueve la licencia allí con un código enviado al correo con el que pagaste, así nadie puede usar tu licencia solo por conocer tu correo, y nunca necesitas contactarnos para cambiar de Mac. El ordenador anterior deja de exportar la próxima vez que se conecta.</p>'),
    ('<h3>Is there a subscription?</h3>', '<h3>¿Hay suscripción?</h3>'),
    ('<p>No. You pay US$49 once and the key never expires. If you set up a new computer, enter the same email, press Activate and type the emailed code; it activates without a new purchase.</p>',
     '<p>No. Pagas 49 US$ una vez y la clave nunca caduca. Si configuras un ordenador nuevo, escribe el mismo correo, pulsa Activar e introduce el código que recibes; se activa sin una nueva compra.</p>'),
    ('<h3>Can I get a refund?</h3>', '<h3>¿Puedo pedir un reembolso?</h3>'),
    ('<p>Within 24 hours: the Creator license, no questions asked; Pro and top-ups, if none of their agent credits have been used. Cancel Pro any time. Email <a href="mailto:license@screenforge.studio">license@screenforge.studio</a> from the address you paid with.</p>',
     '<p>En 24 horas: la licencia Creator, sin preguntas; Pro y las recargas, si no se ha usado ninguno de sus créditos. Cancela Pro cuando quieras. Escribe a <a href="mailto:license@screenforge.studio">license@screenforge.studio</a> desde el correo con el que pagaste.</p>'),
    ('<h3>Why pay if the source is open?</h3>', '<h3>¿Por qué pagar si el código es abierto?</h3>'),
    ("<p>You don't have to. The export gate is plain Apache-2.0 code and a source build can drop it. The license pays for what a source build doesn't give you: the signed and notarized download that opens without warnings, the CI that builds and tests every release, and the time to keep it all working.</p>",
     '<p>No tienes por qué. El bloqueo de exportación es código Apache-2.0 normal y una compilación desde el código puede quitarlo. La licencia paga lo que esa compilación no te da: la descarga firmada y notarizada que se abre sin avisos, la integración continua que compila y prueba cada versión, y el tiempo para mantenerlo todo funcionando.</p>'),
    ('<h3>Do you have team or studio pricing?</h3>', '<h3>¿Tenéis precios para equipos o estudios?</h3>'),
    ('<p>Yes. For five seats or more, email <a href="mailto:license@screenforge.studio?subject=ScreenForge%20for%20our%20studio">license@screenforge.studio</a> with how many people record. You get volume pricing, one invoice and a key for each teammate. See <a href="#studios">Studios &amp; teams</a>.</p>',
     '<p>Sí. Para cinco puestos o más, escribe a <a href="mailto:license@screenforge.studio?subject=ScreenForge%20para%20nuestro%20estudio">license@screenforge.studio</a> indicando cuántas personas graban. Obtienes precio por volumen, una sola factura y una clave para cada persona. Consulta <a href="#studios">Estudios y equipos</a>.</p>'),
    ('<h3>Do I need an account?</h3>', '<h3>¿Necesito una cuenta?</h3>'),
    ('<p>No. There is nothing to sign up for and nothing to sign in to. The email you pay with is the only identifier, and it exists so the key can reach you.</p>',
     '<p>No. No hay nada en lo que registrarse ni donde iniciar sesión. El correo con el que pagas es el único identificador, y existe para que la clave pueda llegarte.</p>'),
    ('<h3>Does the app phone home?</h3>', '<h3>¿La app envía datos a casa?</h3>'),
    ('<p>Only when you press Activate or Buy, to send your email (and, for Activate, an anonymous code for the computer) to the license server; when you open a project on an activated computer, to check the license has not moved; the first time you make captions, to download a speech model, and once a day to ask GitHub whether a newer version is out (you can turn that off). Neither of the last two sends anything about you. There is no telemetry, no usage analytics and no crash reporting. Recordings and exports never leave your computer.</p>',
     '<p>Solo cuando pulsas Activar o Comprar, para enviar tu correo (y, al activar, un código anónimo del ordenador) al servidor de licencias; cuando abres un proyecto en un ordenador activado, para comprobar que la licencia no se ha movido; la primera vez que generas subtítulos, para descargar un modelo de voz, y una vez al día para preguntar a GitHub si hay una versión nueva (puedes desactivarlo). Ninguna de las dos últimas envía nada sobre ti. No hay telemetría, ni analíticas de uso, ni informes de fallos. Las grabaciones y exportaciones nunca salen de tu ordenador.</p>'),
    ("btn.disabled = true; btn.textContent = 'Opening checkout…';", "btn.disabled = true; btn.textContent = 'Abriendo el pago…';"),
    ("show('Could not start checkout (' + ex.message + '). Try again in a moment, or email license@screenforge.studio.');",
     "show('No se pudo iniciar el pago (' + ex.message + '). Inténtalo de nuevo en un momento o escribe a license@screenforge.studio.');"),
    ("l.textContent = 'Download for Linux';", "l.textContent = 'Descargar para Linux';"),
]

# ── download ────────────────────────────────────────────────────────────────
DOWNLOAD_TITLE = 'Descargar ScreenForge — editor de vídeo con IA gratis para macOS, Windows y Linux'
DOWNLOAD_DESC = 'Descarga ScreenForge para macOS, Windows o Linux: grabador de pantalla y cámara gratuito y editor de vídeo con IA para tutoriales, demos de producto y vídeos a cámara.'
LICENSE_P = 'The app runs without a license. Recording and editing are free; exporting needs a key from the <a href="pricing.html">pricing page</a>.'
LICENSE_P_ES = 'La app funciona sin licencia. Grabar y editar es gratis; exportar necesita una clave de la <a href="pricing.html">página de precios</a>.'
DOWNLOAD = [
    ('<div class="eyebrow">Download</div>', '<div class="eyebrow">Descargar</div>'),
    ('role="group" aria-label="Platform"', 'role="group" aria-label="Plataforma"'),
    ('<h1>Download ScreenForge for <em>macOS.</em></h1>', '<h1>Descarga ScreenForge para <em>macOS.</em></h1>'),
    ('<h1>Download ScreenForge for <em>Linux.</em></h1>', '<h1>Descarga ScreenForge para <em>Linux.</em></h1>'),
    ('<h1>Download ScreenForge for <em>Windows.</em></h1>', '<h1>Descarga ScreenForge para <em>Windows.</em></h1>'),
    ('<p class="stamp">Apple silicon · macOS 14 or later · recording and editing are free</p>', '<p class="stamp">Apple silicon · macOS 14 o posterior · grabar y editar es gratis</p>'),
    ('<p class="stamp">x86_64 · ffmpeg included · recording and editing are free</p>', '<p class="stamp">x86_64 · ffmpeg incluido · grabar y editar es gratis</p>'),
    ('<p class="stamp">x86_64 · Windows 10 or later · ffmpeg included · recording and editing are free</p>', '<p class="stamp">x86_64 · Windows 10 o posterior · ffmpeg incluido · grabar y editar es gratis</p>'),
    ('aria-label="requirements"', 'aria-label="requisitos"', 3),
    ('<span class="k">macOS</span><span class="v">14 or later</span>', '<span class="k">macOS</span><span class="v">14 o posterior</span>'),
    ('<span class="k">size</span><span class="v" id="dl-size">a few MB</span>', '<span class="k">tamaño</span><span class="v" id="dl-size">unos MB</span>'),
    ('<span class="k">status</span><span class="v">beta</span>', '<span class="k">estado</span><span class="v">beta</span>', 3),
    ('<span class="k">arch</span><span class="v">x86_64</span>', '<span class="k">arquitectura</span><span class="v">x86_64</span>', 2),
    ('<span class="k">session</span><span class="v">X11 or Wayland</span>', '<span class="k">sesión</span><span class="v">X11 o Wayland</span>'),
    ('<span class="k">ffmpeg</span><span class="v">included</span>', '<span class="k">ffmpeg</span><span class="v">incluido</span>', 2),
    ('<span class="k">Windows</span><span class="v">10 (2004) or later</span>', '<span class="k">Windows</span><span class="v">10 (2004) o posterior</span>'),
    ('      Download .dmg\n', '      Descargar .dmg\n'),
    ('      Download .deb\n', '      Descargar .deb\n'),
    ('      Download .tar.gz\n', '      Descargar .tar.gz\n'),
    ('      Download installer\n', '      Descargar instalador\n'),
    ('      Portable .zip\n', '      .zip portable\n'),
    ('aria-live="polite">finding the latest release…</span>', 'aria-live="polite">buscando la última versión…</span>'),
    ('aria-live="polite">Debian / Ubuntu: .deb · other distros: .tar.gz</span>', 'aria-live="polite">Debian / Ubuntu: .deb · otras distribuciones: .tar.gz</span>'),
    ('aria-live="polite">installer: Start menu and uninstall · zip: run from any folder</span>', 'aria-live="polite">instalador: menú Inicio y desinstalación · zip: ejecútalo desde cualquier carpeta</span>'),
    ('<h2>Install</h2>', '<h2>Instalación</h2>', 3),
    ('<li><b>Step 1</b><strong>Open the DMG</strong>Double-click the file you just downloaded. A window opens with the app and a shortcut to Applications.</li>',
     '<li><b>Paso 1</b><strong>Abre el DMG</strong>Haz doble clic en el archivo que acabas de descargar. Se abre una ventana con la app y un acceso directo a Aplicaciones.</li>'),
    ('<li><b>Step 2</b><strong>Drag to Applications</strong>Drop ScreenForge on the Applications folder, eject the disk image, launch it from Applications or Spotlight.</li>',
     '<li><b>Paso 2</b><strong>Arrastra a Aplicaciones</strong>Suelta ScreenForge en la carpeta Aplicaciones, expulsa la imagen de disco y ábrela desde Aplicaciones o Spotlight.</li>'),
    ('<li><b>Step 3</b><strong>Grant two permissions</strong>On first launch macOS asks for <i class="hl">Screen Recording</i> and <i class="hl">Input Monitoring</i>. Both live in System Settings → Privacy &amp; Security. Recording needs the first; automatic zooms, which follow your clicks and typing, need the second.</li>',
     '<li><b>Paso 3</b><strong>Concede dos permisos</strong>En el primer arranque macOS pide <i class="hl">Grabación de pantalla</i> y <i class="hl">Monitorización de entrada</i>. Ambos están en Ajustes del Sistema → Privacidad y seguridad. Grabar necesita el primero; los zooms automáticos, que siguen tus clics y tu escritura, necesitan el segundo.</li>'),
    (LICENSE_P, LICENSE_P_ES, 3),
    ('<p style="margin-top:28px">On Windows? <a href="#windows" data-os-link="windows">Get the Windows version</a>. On Linux? <a href="#linux" data-os-link="linux">Get the Linux version</a>.</p>',
     '<p style="margin-top:28px">¿Usas Windows? <a href="#windows" data-os-link="windows">Descarga la versión para Windows</a>. ¿Linux? <a href="#linux" data-os-link="linux">Descarga la versión para Linux</a>.</p>'),
    ('<li><b>Step 1</b><strong>Install the package</strong>From the folder you downloaded to, run <code>sudo apt install ./screenforge_amd64.deb</code>, or open it in your software center. <i class="hl">ffmpeg</i> comes bundled; nothing else to install.</li>',
     '<li><b>Paso 1</b><strong>Instala el paquete</strong>Desde la carpeta de descarga, ejecuta <code>sudo apt install ./screenforge_amd64.deb</code>, o ábrelo en tu centro de software. <i class="hl">ffmpeg</i> viene incluido; no hay nada más que instalar.</li>'),
    ('<li><b>Step 2</b><strong>Share your screen</strong>Recording works on <i class="hl">X11</i> and <i class="hl">Wayland</i>. On Wayland (GNOME, KDE Plasma) your first recording opens the system\'s "Share your screen" dialog: pick your screen and click Share. ScreenForge remembers the choice and only asks again for a different monitor. To record a window, pick it on the dialog\'s Window tab. On Wayland the cursor is part of the video and clicks and keystrokes are not recorded, so automatic zooms and cursor effects are not available there yet.</li>',
     '<li><b>Paso 2</b><strong>Comparte tu pantalla</strong>La grabación funciona en <i class="hl">X11</i> y <i class="hl">Wayland</i>. En Wayland (GNOME, KDE Plasma) la primera grabación abre el diálogo «Compartir pantalla» del sistema: elige tu pantalla y pulsa Compartir. ScreenForge recuerda la elección y solo vuelve a preguntar para otro monitor. Para grabar una ventana, elígela en la pestaña Ventana del diálogo. En Wayland el cursor forma parte del vídeo y los clics y pulsaciones no se registran, así que los zooms automáticos y los efectos de cursor aún no están disponibles ahí.</li>'),
    ('<li><b>Step 3</b><strong>Launch</strong>Open ScreenForge from your app menu, or run <code>screenforge</code> in a terminal. System audio recording is not available on Linux yet; the microphone works.</li>',
     '<li><b>Paso 3</b><strong>Ábrela</strong>Abre ScreenForge desde el menú de aplicaciones o ejecuta <code>screenforge</code> en una terminal. La grabación del audio del sistema aún no está disponible en Linux; el micrófono funciona.</li>'),
    ('<span class="c"># Debian / Ubuntu</span>', '<span class="c"># Debian / Ubuntu</span>'),
    ('<span class="c"># other distros</span>', '<span class="c"># otras distribuciones</span>'),
    ('<span class="c"># Wayland: no "Share your screen" dialog? restart the portal</span>', '<span class="c"># Wayland: ¿no aparece el diálogo «Compartir pantalla»? reinicia el portal</span>'),
    ('<p style="margin-top:28px">On a Mac? <a href="#mac" data-os-link="mac">Get the macOS version</a>. On Windows? <a href="#windows" data-os-link="windows">Get the Windows version</a>.</p>',
     '<p style="margin-top:28px">¿Usas Mac? <a href="#mac" data-os-link="mac">Descarga la versión para macOS</a>. ¿Windows? <a href="#windows" data-os-link="windows">Descarga la versión para Windows</a>.</p>'),
    ('<li><b>Step 1</b><strong>Run the installer</strong>It installs for your user account only, so no administrator prompt. The installer is signed by Smartloop Inc. If your browser or SmartScreen still warns about a rarely downloaded file, choose <i class="hl">Keep</i> and then <i class="hl">More info</i> → <i class="hl">Run anyway</i>; the warning goes away as the certificate earns reputation. Prefer no install? Unzip the portable build anywhere and run <code>screenforge.exe</code>.</li>',
     '<li><b>Paso 1</b><strong>Ejecuta el instalador</strong>Se instala solo para tu cuenta de usuario, así que no pide permisos de administrador. El instalador está firmado por Smartloop Inc. Si tu navegador o SmartScreen aún avisa de un archivo poco descargado, elige <i class="hl">Conservar</i> y luego <i class="hl">Más información</i> → <i class="hl">Ejecutar de todas formas</i>; el aviso desaparece a medida que el certificado gana reputación. ¿Prefieres no instalar? Descomprime la versión portable donde quieras y ejecuta <code>screenforge.exe</code>.</li>'),
    ('<li><b>Step 2</b><strong>Record</strong>No permissions to grant: the screen is captured through the Desktop Duplication API, and the cursor, clicks and keystrokes are recorded separately so the editor can smooth and zoom them. System audio and the microphone both work.</li>',
     '<li><b>Paso 2</b><strong>Graba</strong>No hay permisos que conceder: la pantalla se captura con la API Desktop Duplication, y el cursor, los clics y las pulsaciones se registran aparte para que el editor pueda suavizarlos y hacer zoom. El audio del sistema y el micrófono funcionan.</li>'),
    ('<li><b>Step 3</b><strong>Launch</strong>Open ScreenForge from the Start menu. <i class="hl">ffmpeg</i> comes bundled; nothing else to install.</li>',
     '<li><b>Paso 3</b><strong>Ábrela</strong>Abre ScreenForge desde el menú Inicio. <i class="hl">ffmpeg</i> viene incluido; no hay nada más que instalar.</li>'),
    ('<p style="margin-top:28px">On a Mac? <a href="#mac" data-os-link="mac">Get the macOS version</a>. On Linux? <a href="#linux" data-os-link="linux">Get the Linux version</a>.</p>',
     '<p style="margin-top:28px">¿Usas Mac? <a href="#mac" data-os-link="mac">Descarga la versión para macOS</a>. ¿Linux? <a href="#linux" data-os-link="linux">Descarga la versión para Linux</a>.</p>'),
    ('<h2>Older versions</h2>', '<h2>Versiones anteriores</h2>'),
    ('<p>Every build, with its notes and checksums, is on <a href="https://github.com/mehfuzh/screenforge/releases">GitHub Releases</a>. The download buttons above always point at the newest one.</p>',
     '<p>Todas las versiones, con sus notas y sumas de verificación, están en <a href="https://github.com/mehfuzh/screenforge/releases">GitHub Releases</a>. Los botones de descarga de arriba siempre apuntan a la más reciente.</p>'),
    ("m.textContent = 'latest release on GitHub';", "m.textContent = 'última versión en GitHub';"),
    ("d.toLocaleDateString('en-GB',", "d.toLocaleDateString('es-ES',"),
]

# ── comparison ──────────────────────────────────────────────────────────────
COMPARISON_TITLE = 'Jam vs Screen Studio vs ScreenForge — ScreenForge'
COMPARISON_OG_TITLE = 'Jam vs Screen Studio vs ScreenForge'
COMPARISON_DESC = 'Jam, Screen Studio y ScreenForge comparados en plataformas, motor, funciones de edición, voz en off, música, fondos, IA y precio. Escrito por el autor de ScreenForge, con sus carencias incluidas. Una alternativa a Screen Studio para Windows y Linux.'
Y, P, N = '<span class="y">Yes</span>', '<span class="p">', '<span class="n">No</span>'
COMPARISON = [
    ('<div class="eyebrow">Comparison</div>', '<div class="eyebrow">Comparativa</div>'),
    ('<p class="stamp">Checked 31 October 2026</p>', '<p class="stamp">Revisado el 31 de octubre de 2026</p>'),
    ('<div class="k">The short version</div>', '<div class="k">La versión corta</div>'),
    ('<p>Three tools that all record your screen, built for three different jobs. <strong>Jam</strong> is a bug reporter for software teams: it attaches console and network logs to a recording. <strong>Screen Studio</strong> is a polished demo recorder for the Mac. <strong>ScreenForge</strong> is an open source recorder and editor with an AI agent that edits the timeline from a prompt, on Mac, Windows and Linux. This page is written by the ScreenForge author, so the gaps are listed too.</p>',
     '<p>Tres herramientas que graban la pantalla, pensadas para tres trabajos distintos. <strong>Jam</strong> es un reportero de errores para equipos de software: adjunta registros de consola y de red a la grabación. <strong>Screen Studio</strong> es un grabador de demos muy pulido para Mac. <strong>ScreenForge</strong> es un grabador y editor de código abierto con un agente de IA que edita la línea de tiempo a partir de una petición, en Mac, Windows y Linux. Esta página la escribe el autor de ScreenForge, así que también se listan sus carencias.</p>'),
    ('<div class="key"><span><span class="y">Yes</span> built in</span><span><span class="p">Partly</span> with a caveat</span><span><span class="n">No</span> not offered</span></div>',
     '<div class="key"><span><span class="y">Sí</span> incluido</span><span><span class="p">En parte</span> con matices</span><span><span class="n">No</span> no lo ofrece</span></div>'),
    ('<tr><th>Pick it if</th><td>You file bugs and want console and network logs attached</td><td>You want the most polished Mac demo with the least effort</td><td class="y">You want to make the most of AI, all-in-one and cross-platform</td></tr>',
     '<tr><th>Elígelo si</th><td>Reportas errores y quieres los registros de consola y de red adjuntos</td><td>Quieres la demo más pulida en Mac con el mínimo esfuerzo</td><td class="y">Quieres sacar el máximo partido a la IA, todo en uno y multiplataforma</td></tr>'),
    ('<tr class="sec"><th colspan="4">Platforms</th></tr>', '<tr class="sec"><th colspan="4">Plataformas</th></tr>'),
    ('<tr><th>macOS</th><td><span class="y">Yes</span><span class="note">Mac app, up to 4K</span></td><td><span class="y">Yes</span><span class="note">macOS 13.1+, Apple silicon</span></td><td><span class="y">Yes</span><span class="note">macOS 14+, Apple silicon</span></td></tr>',
     '<tr><th>macOS</th><td><span class="y">Sí</span><span class="note">app para Mac, hasta 4K</span></td><td><span class="y">Sí</span><span class="note">macOS 13.1+, Apple silicon</span></td><td><span class="y">Sí</span><span class="note">macOS 14+, Apple silicon</span></td></tr>'),
    ('<tr><th>Windows</th><td><span class="p">Browser only</span><span class="note">Chrome extension, no desktop app</span></td><td><span class="n">No</span><span class="note">"no near-future plans" per their FAQ</span></td><td><span class="y">Yes</span><span class="note">Windows 10+, cursor, clicks, keys and system audio recorded</span></td></tr>',
     '<tr><th>Windows</th><td><span class="p">Solo navegador</span><span class="note">extensión de Chrome, sin app de escritorio</span></td><td><span class="n">No</span><span class="note">«sin planes a corto plazo», según su FAQ</span></td><td><span class="y">Sí</span><span class="note">Windows 10+, graba cursor, clics, teclas y audio del sistema</span></td></tr>'),
    ('<tr><th>Linux</th><td><span class="p">Browser only</span><span class="note">Chrome extension</span></td><td><span class="n">No</span></td><td><span class="y">Yes</span><span class="note">X11 and Wayland. On Wayland clicks and keys are not captured, so no auto zoom there yet</span></td></tr>',
     '<tr><th>Linux</th><td><span class="p">Solo navegador</span><span class="note">extensión de Chrome</span></td><td><span class="n">No</span></td><td><span class="y">Sí</span><span class="note">X11 y Wayland. En Wayland no se capturan clics ni teclas, así que aún no hay zoom automático ahí</span></td></tr>'),
    ('<tr><th>iPhone / iPad</th><td><span class="y">iOS app</span><span class="note">records on the phone itself</span></td><td><span class="y">Over USB</span><span class="note">device mockups applied</span></td><td><span class="y">Over USB</span><span class="note">drawn device frames</span></td></tr>',
     '<tr><th>iPhone / iPad</th><td><span class="y">App iOS</span><span class="note">graba en el propio teléfono</span></td><td><span class="y">Por USB</span><span class="note">con maquetas de dispositivo</span></td><td><span class="y">Por USB</span><span class="note">marcos de dispositivo dibujados</span></td></tr>'),
    ('<tr><th>Open source</th><td><span class="n">No</span></td><td><span class="n">No</span></td><td><span class="y">Apache-2.0</span><span class="note">100% Rust, build it yourself</span></td></tr>',
     '<tr><th>Código abierto</th><td><span class="n">No</span></td><td><span class="n">No</span></td><td><span class="y">Apache-2.0</span><span class="note">100% Rust, compílalo tú mismo</span></td></tr>'),
    ('<tr class="sec"><th colspan="4">Performance and engine</th></tr>', '<tr class="sec"><th colspan="4">Rendimiento y motor</th></tr>'),
    ('<tr><th>App stack</th><td>Chrome extension + web app; Mac app. Processing and sharing happen in the cloud</td><td>Native ScreenCaptureKit capture. Third-party reviews describe the UI as Electron; not confirmed by the vendor</td><td>Native Rust app, egui UI, wgpu GPU compositor, ffmpeg encode</td></tr>',
     '<tr><th>Tecnología</th><td>Extensión de Chrome + app web; app para Mac. El procesado y la compartición ocurren en la nube</td><td>Captura nativa con ScreenCaptureKit. Reseñas de terceros describen la interfaz como Electron; no confirmado por el fabricante</td><td>App nativa en Rust, interfaz egui, compositor GPU wgpu, codificación con ffmpeg</td></tr>'),
    ('<tr><th>Capture API</th><td>Browser capture; ScreenCaptureKit on Mac</td><td>ScreenCaptureKit</td><td>ScreenCaptureKit (Mac), Desktop Duplication (Windows), PipeWire portal (Linux)</td></tr>',
     '<tr><th>API de captura</th><td>Captura del navegador; ScreenCaptureKit en Mac</td><td>ScreenCaptureKit</td><td>ScreenCaptureKit (Mac), Desktop Duplication (Windows), portal PipeWire (Linux)</td></tr>'),
    ('<tr><th>Local vs cloud</th><td>Cloud first: videos live on jam.dev</td><td>Local editing and export; optional share links</td><td>Local editing and export. Only the AI agent and ElevenLabs voices call a server</td></tr>',
     '<tr><th>Local o nube</th><td>Nube primero: los vídeos viven en jam.dev</td><td>Edición y exportación locales; enlaces para compartir opcionales</td><td>Edición y exportación locales. Solo el agente de IA y las voces de ElevenLabs llaman a un servidor</td></tr>'),
    ('<tr><th>Upscaling</th><td><span class="n">No</span></td><td><span class="y">6.5× on-device upscaler</span></td><td><span class="n">No</span></td></tr>',
     '<tr><th>Escalado</th><td><span class="n">No</span></td><td><span class="y">Escalador 6,5× en el dispositivo</span></td><td><span class="n">No</span></td></tr>'),
    ('<tr><th>Maturity</th><td>Mature, 300k+ users</td><td>Mature, years of releases</td><td><span class="p">Beta</span><span class="note">no public benchmarks yet</span></td></tr>',
     '<tr><th>Madurez</th><td>Madura, más de 300.000 usuarios</td><td>Madura, años de versiones</td><td><span class="p">Beta</span><span class="note">aún sin pruebas de rendimiento públicas</span></td></tr>'),
    ('<tr class="sec"><th colspan="4">Editing features</th></tr>', '<tr class="sec"><th colspan="4">Funciones de edición</th></tr>'),
    ('<tr><th>Auto zoom</th><td><span class="n">No</span></td><td><span class="y">Yes</span><span class="note">classic + loupe styles, smart framing</span></td><td><span class="y">Yes</span><span class="note">follows clicks and the typing caret (caret zoom Mac only)</span></td></tr>',
     '<tr><th>Zoom automático</th><td><span class="n">No</span></td><td><span class="y">Sí</span><span class="note">estilos clásico y lupa, encuadre inteligente</span></td><td><span class="y">Sí</span><span class="note">sigue los clics y el cursor de texto (zoom al cursor de texto solo en Mac)</span></td></tr>'),
    ('<tr><th>Cursor</th><td><span class="n">No effects</span></td><td><span class="y">Smooth, motion blur, click effects, resize, hide idle</span></td><td><span class="p">Smooth, tilt, click effects, cursor sets</span><span class="note">motion blur setting exists but is not rendered yet</span></td></tr>',
     '<tr><th>Cursor</th><td><span class="n">Sin efectos</span></td><td><span class="y">Suavizado, desenfoque de movimiento, efectos de clic, tamaño, ocultar inactivo</span></td><td><span class="p">Suavizado, inclinación, efectos de clic, juegos de cursores</span><span class="note">el ajuste de desenfoque de movimiento existe pero aún no se renderiza</span></td></tr>'),
    ('<tr><th>Camera</th><td><span class="p">Camera bubble</span></td><td><span class="y">Layouts, background cutout, face tracking, hide in silence</span></td><td><span class="p">Layouts, face tracking, Resolve-style colour grade, .cube LUTs</span><span class="note">no background cutout</span></td></tr>',
     '<tr><th>Cámara</th><td><span class="p">Burbuja de cámara</span></td><td><span class="y">Diseños, recorte de fondo, seguimiento facial, ocultar en silencios</span></td><td><span class="p">Diseños, seguimiento facial, etalonaje al estilo Resolve, LUTs .cube</span><span class="note">sin recorte de fondo</span></td></tr>'),
    ('<tr><th>Captions</th><td><span class="y">Transcript, captions, chapters</span><span class="note">cloud</span></td><td><span class="y">On-device Whisper or Apple Speech, styled, SRT</span></td><td><span class="y">On-device Whisper, styled, SRT + WebVTT</span></td></tr>',
     '<tr><th>Subtítulos</th><td><span class="y">Transcripción, subtítulos, capítulos</span><span class="note">en la nube</span></td><td><span class="y">Whisper o Apple Speech en el dispositivo, con estilos, SRT</span></td><td><span class="y">Whisper en el dispositivo, con estilos, SRT + WebVTT</span></td></tr>'),
    ('<tr><th>Edit via transcript</th><td><span class="n">No</span></td><td><span class="y">Yes</span><span class="note">delete text to cut, remove fillers and pauses</span></td><td><span class="p">By prompt only</span><span class="note">"cut the ums and pauses" works; no text editor yet</span></td></tr>',
     '<tr><th>Edición por transcripción</th><td><span class="n">No</span></td><td><span class="y">Sí</span><span class="note">borra texto para cortar, quita muletillas y pausas</span></td><td><span class="p">Solo por petición</span><span class="note">«quita los eh y las pausas» funciona; aún sin editor de texto</span></td></tr>'),
    ('<tr><th>Blur / masks</th><td><span class="y">Auto-blur</span><span class="note">plan dependent</span></td><td><span class="n">Not listed</span></td><td><span class="y">Blur and highlight masks on a timeline lane</span></td></tr>',
     '<tr><th>Difuminado / máscaras</th><td><span class="y">Difuminado automático</span><span class="note">según el plan</span></td><td><span class="n">No aparece</span></td><td><span class="y">Máscaras de difuminado y resaltado en una pista de la línea de tiempo</span></td></tr>'),
    ('<tr><th>Export</th><td><span class="p">Share link</span><span class="note">hosted on jam.dev</span></td><td><span class="y">MP4 to 4K60, GIF, 16:9 / 1:1 / 9:16, share links</span></td><td><span class="p">MP4 H.264/HEVC, ProRes, GIF, WebM, SRT/VTT</span><span class="note">no hosted share links</span></td></tr>',
     '<tr><th>Exportación</th><td><span class="p">Enlace para compartir</span><span class="note">alojado en jam.dev</span></td><td><span class="y">MP4 hasta 4K60, GIF, 16:9 / 1:1 / 9:16, enlaces para compartir</span></td><td><span class="p">MP4 H.264/HEVC, ProRes, GIF, WebM, SRT/VTT</span><span class="note">sin enlaces alojados para compartir</span></td></tr>'),
    ('<tr><th>Dev context</th><td><span class="y">Console + network logs, device info, Jira / Linear / Slack / GitHub</span></td><td><span class="n">No</span></td><td><span class="n">No</span></td></tr>',
     '<tr><th>Contexto para desarrollo</th><td><span class="y">Registros de consola y red, datos del dispositivo, Jira / Linear / Slack / GitHub</span></td><td><span class="n">No</span></td><td><span class="n">No</span></td></tr>'),
    ('<tr class="sec"><th colspan="4">Voice-over, music, backgrounds</th></tr>', '<tr class="sec"><th colspan="4">Voz en off, música, fondos</th></tr>'),
    ('<tr><th>Voice-over</th><td><span class="p">Narrate while recording</span></td><td><span class="y">Mic, AI voice from script, noise reduction</span></td><td><span class="y">Record over the video afterwards, AI narration, +dB boost</span><span class="note">no noise reduction</span></td></tr>',
     '<tr><th>Voz en off</th><td><span class="p">Narra mientras grabas</span></td><td><span class="y">Micrófono, voz de IA desde guion, reducción de ruido</span></td><td><span class="y">Graba sobre el vídeo después, narración con IA, ganancia en dB</span><span class="note">sin reducción de ruido</span></td></tr>'),
    ('<tr><th>AI voices</th><td><span class="n">No</span></td><td><span class="y">AI voiceover</span></td><td><span class="y">6 on-device voices (Kokoro) + 8 ElevenLabs voices</span><span class="note">ElevenLabs needs Pro</span></td></tr>',
     '<tr><th>Voces de IA</th><td><span class="n">No</span></td><td><span class="y">Voz en off con IA</span></td><td><span class="y">6 voces en el dispositivo (Kokoro) + 8 voces de ElevenLabs</span><span class="note">ElevenLabs requiere Pro</span></td></tr>'),
    ('<tr><th>Background music</th><td><span class="n">No</span></td><td><span class="y">Yes</span></td><td><span class="y">Built-in library or your own files, own audio track</span></td></tr>',
     '<tr><th>Música de fondo</th><td><span class="n">No</span></td><td><span class="y">Sí</span></td><td><span class="y">Biblioteca integrada o tus propios archivos, pista de audio propia</span></td></tr>'),
    ("<tr><th>Wallpapers</th><td><span class=\"n\">No</span></td><td><span class=\"y\">macOS wallpapers, gradients, your images, Unsplash</span></td><td><span class=\"y\">Built-in, your OS's wallpapers (Mac / Ubuntu / Windows), your images, Unsplash search</span></td></tr>",
     '<tr><th>Fondos</th><td><span class="n">No</span></td><td><span class="y">Fondos de macOS, degradados, tus imágenes, Unsplash</span></td><td><span class="y">Integrados, los fondos de tu sistema (Mac / Ubuntu / Windows), tus imágenes, búsqueda en Unsplash</span></td></tr>'),
    ('<tr class="sec"><th colspan="4">AI</th></tr>', '<tr class="sec"><th colspan="4">IA</th></tr>'),
    ('<tr><th>What AI does</th><td>Writes a title, summary and repro steps for each bug</td><td>AI voice-over, transcript-based editing, on-device transcription</td><td>An editing agent (Claude) edits the timeline from a prompt: cuts, speed, zooms, masks, music, narration, backgrounds. Plus narration and captions</td></tr>',
     '<tr><th>Qué hace la IA</th><td>Escribe título, resumen y pasos para reproducir cada error</td><td>Voz en off con IA, edición por transcripción, transcripción en el dispositivo</td><td>Un agente de edición (Claude) edita la línea de tiempo desde una petición: cortes, velocidad, zooms, máscaras, música, narración, fondos. Además de narración y subtítulos</td></tr>'),
    ('<tr><th>AI for agents</th><td><span class="y">MCP server</span><span class="note">coding agents read logs and video</span></td><td><span class="n">No</span></td><td><span class="n">No MCP</span><span class="note">the agent is inside the app</span></td></tr>',
     '<tr><th>IA para agentes</th><td><span class="y">Servidor MCP</span><span class="note">los agentes de programación leen registros y vídeo</span></td><td><span class="n">No</span></td><td><span class="n">Sin MCP</span><span class="note">el agente está dentro de la app</span></td></tr>'),
    ('<tr><th>Runs where</th><td>Cloud</td><td>Mostly on-device; cloud transcription optional</td><td>Captions, Kokoro voices, face tracking on-device; editing agent in the cloud (Pro)</td></tr>',
     '<tr><th>Dónde se ejecuta</th><td>Nube</td><td>Sobre todo en el dispositivo; transcripción en la nube opcional</td><td>Subtítulos, voces Kokoro y seguimiento facial en el dispositivo; agente de edición en la nube (Pro)</td></tr>'),
    ('<tr><th>AI limits</th><td>30 summaries/mo free, 200 on Team</td><td>Included in subscription</td><td>1,000 credits/mo on Pro (about US$0.01 each), top-ups never expire</td></tr>',
     '<tr><th>Límites de IA</th><td>30 resúmenes/mes gratis, 200 en Team</td><td>Incluido en la suscripción</td><td>1.000 créditos/mes en Pro (unos 0,01 US$ cada uno), las recargas no caducan</td></tr>'),
    ('<tr class="sec"><th colspan="4">Price</th></tr>', '<tr class="sec"><th colspan="4">Precio</th></tr>'),
    ('<tr><th>Free tier</th><td><span class="y">30 Jams/mo, 5-min recordings</span></td><td><span class="n">14-day trial only</span></td><td><span class="y">Record and edit free</span><span class="note">export needs a licence, or build from source</span></td></tr>',
     '<tr><th>Plan gratuito</th><td><span class="y">30 Jams/mes, grabaciones de 5 min</span></td><td><span class="n">Solo prueba de 14 días</span></td><td><span class="y">Graba y edita gratis</span><span class="note">exportar necesita licencia, o compila desde el código</span></td></tr>'),
    ('<tr><th>Paid</th><td>Team <span class="p">US$14 / mo</span> per creator, billed yearly</td><td><span class="p">US$29 / mo</span><span class="note">monthly plan, third-party listings, Oct 2026</span></td><td>Creator US$49 once (export) or Pro <span class="p">US$20 / mo</span> (export + AI agent)</td></tr>',
     '<tr><th>De pago</th><td>Team <span class="p">14 US$ / mes</span> por creador, facturado anualmente</td><td><span class="p">29 US$ / mes</span><span class="note">plan mensual, listados de terceros, oct. 2026</span></td><td>Creator 49 US$ una vez (exportación) o Pro <span class="p">20 US$ / mes</span> (exportación + agente de IA)</td></tr>'),
    ('<div><h3>Pick Jam if</h3><ul><li>You file bugs and want console and network logs attached automatically.</li><li>Your team lives in Jira, Linear or Slack.</li><li>You never need to polish the video.</li></ul></div>',
     '<div><h3>Elige Jam si</h3><ul><li>Reportas errores y quieres los registros de consola y de red adjuntos automáticamente.</li><li>Tu equipo vive en Jira, Linear o Slack.</li><li>Nunca necesitas pulir el vídeo.</li></ul></div>'),
    ('<div><h3>Pick Screen Studio if</h3><ul><li>You are on a Mac and want the most refined look with the least effort.</li><li>You need camera background cutout, transcript editing or 4K upscaling.</li><li>You want a tool that has been shipping for years.</li></ul></div>',
     '<div><h3>Elige Screen Studio si</h3><ul><li>Usas Mac y quieres el acabado más refinado con el mínimo esfuerzo.</li><li>Necesitas recorte de fondo de cámara, edición por transcripción o escalado a 4K.</li><li>Quieres una herramienta que lleva años publicándose.</li></ul></div>'),
    ('<div><h3>Pick ScreenForge if</h3><ul><li>You want to make the most of AI: type "cut the pauses, zoom on clicks, add music and narration" and have it done.</li><li>You want one cross-platform app for recording, editing, captions, voice-over, music and export, on Mac, Windows and Linux.</li><li>You want open source and local-first, and are fine with a beta.</li></ul></div>',
     '<div><h3>Elige ScreenForge si</h3><ul><li>Quieres sacar el máximo partido a la IA: escribe «quita las pausas, haz zoom en los clics, añade música y narración» y queda hecho.</li><li>Quieres una sola app multiplataforma para grabar, editar, subtítulos, voz en off, música y exportación, en Mac, Windows y Linux.</li><li>Quieres código abierto y local primero, y te vale una beta.</li></ul></div>'),
    ('<h2>Sources</h2>', '<h2>Fuentes</h2>'),
    ('<p><a href="https://jam.dev/pricing">jam.dev/pricing</a>, <a href="https://jam.dev/mac">jam.dev/mac</a>, <a href="https://screen.studio/">screen.studio</a>, <a href="https://screen.studio/guides/screen-capture-kit-issues">Screen Studio\'s ScreenCaptureKit guide</a>, and the ScreenForge <a href="https://github.com/mehfuzh/screenforge">README and status notes</a>. Screen Studio\'s price comes from third-party listings because its pricing page did not load; check before quoting. No speed benchmarks were run for this table. Spotted a mistake? Open an issue on <a href="https://github.com/mehfuzh/screenforge/issues">GitHub</a> and it will be fixed.</p>',
     '<p><a href="https://jam.dev/pricing">jam.dev/pricing</a>, <a href="https://jam.dev/mac">jam.dev/mac</a>, <a href="https://screen.studio/">screen.studio</a>, <a href="https://screen.studio/guides/screen-capture-kit-issues">la guía de ScreenCaptureKit de Screen Studio</a> y el <a href="https://github.com/mehfuzh/screenforge">README y las notas de estado</a> de ScreenForge. El precio de Screen Studio procede de listados de terceros porque su página de precios no cargó; compruébalo antes de citarlo. No se hicieron pruebas de velocidad para esta tabla. ¿Has visto un error? Abre una incidencia en <a href="https://github.com/mehfuzh/screenforge/issues">GitHub</a> y se corregirá.</p>'),
]

META = {
    'index.html': dict(title=INDEX_TITLE, desc=INDEX_DESC, og_title=INDEX_TITLE, path='', table=INDEX, jsonld=INDEX_JSONLD),
    'pricing.html': dict(title=PRICING_TITLE, desc=PRICING_DESC, og_title=PRICING_TITLE, path='pricing', table=PRICING),
    'download.html': dict(title=DOWNLOAD_TITLE, desc=DOWNLOAD_DESC, og_title=DOWNLOAD_TITLE, path='download', table=DOWNLOAD),
    'comparison.html': dict(title=COMPARISON_TITLE, desc=COMPARISON_DESC, og_title=COMPARISON_OG_TITLE, path='comparison', table=COMPARISON),
}


def english_hrefs(page):
    return ('/', '/es/') if page == 'index.html' else (page, 'es/' + page)


def spanish_hrefs(page):
    return ('/', '/es/') if page == 'index.html' else ('../' + page, page)


def prepare_english(page, s):
    """Flag switch, hreflang and locale tags on the English page (idempotent)."""
    en_url = 'https://screenforge.studio/' + META[page]['path']
    es_url = 'https://screenforge.studio/es/' + META[page]['path']
    if '.lang-opt' not in s:
        s = must(s, "  /* buttons, chips and the platform picker share the switch's corner radius */", LANG_CSS)
        s = must(s, '      <button type="button" class="mode-toggle"', '      ' + lang_switch('en', *english_hrefs(page)))
        s = must(s, '    .nav-links a:not(.nav-cta) { display: none; }', '    .nav-links a:not(.nav-cta):not(.lang-opt) { display: none; }')
        s = must(s, '    paint();\n  })();\n</script>', LANG_JS)
    if "localStorage.getItem('lang')" not in s.split('</head>')[0]:
        s = must(s, '    document.documentElement.dataset.theme = t;\n', EN_BOOT)
    if 'hreflang="es"' not in s.split('</head>')[0]:
        s = must(s, '<meta name="color-scheme" content="light dark">',
                 f'<link rel="alternate" hreflang="en" href="{en_url}">\n<link rel="alternate" hreflang="es" href="{es_url}">\n'
                 f'<link rel="alternate" hreflang="x-default" href="{en_url}">\n<meta name="color-scheme" content="light dark">')
    if 'og:locale' not in s:
        s = must(s, f'<meta property="og:url" content="{en_url}">',
                 f'<meta property="og:url" content="{en_url}">\n<meta property="og:locale" content="en_US">\n<meta property="og:locale:alternate" content="es_ES">')
    return s


def translate(page, en):
    m = META[page]
    en_url = 'https://screenforge.studio/' + m['path']
    es_url = 'https://screenforge.studio/es/' + m['path']
    head, rest = en.split('<style>', 1)
    style, body = rest.split('</head>', 1)
    style = '<style>' + style
    body = '</head>' + body

    # head: language, canonical, titles, descriptions, locale, asset paths
    head = must(head, '<html lang="en">', '<html lang="es">')
    head = must(head, f'<link rel="canonical" href="{en_url}">', f'<link rel="canonical" href="{es_url}">')
    head = must(head, f'<meta property="og:url" content="{en_url}">', f'<meta property="og:url" content="{es_url}">')
    head = must(head, '<meta property="og:locale" content="en_US">\n<meta property="og:locale:alternate" content="es_ES">',
                '<meta property="og:locale" content="es_ES">\n<meta property="og:locale:alternate" content="en_US">')
    en_title = re.search(r'<title>(.*?)</title>', head).group(1)
    en_desc = re.search(r'<meta name="description" content="(.*?)">', head).group(1)
    en_og_title = re.search(r'<meta property="og:title" content="(.*?)">', head).group(1)
    head = head.replace(f'<title>{en_title}</title>', f'<title>{m["title"]}</title>')
    head = head.replace(f'content="{en_desc}"', f'content="{m["desc"]}"')
    head = head.replace(f'content="{en_og_title}"', f'content="{m["og_title"]}"')
    if 'jsonld' in m:
        head = re.sub(r'<script type="application/ld\+json">.*?</script>', lambda _: m['jsonld'], head, count=1, flags=re.S)
    for a in ASSETS:
        head = head.replace(f'href="{a}', f'href="../{a}')

    # styles and head scripts: verbatim, minus the English-only language redirect
    style = maybe(style, "location.replace('success.html'", "location.replace('../success.html'")
    style = must(style, EN_BOOT, ES_BOOT)
    if page == 'index.html':
        style = re.sub(r'(\n  h1 \{[^}]*max-width: )22ch', r'\g<1>24ch', style, count=1)  # the Spanish headline runs longer

    # body: the switch, then the copy
    en_href, es_href = spanish_hrefs(page)
    body = must(body, lang_switch('en', *english_hrefs(page)), lang_switch('es', en_href, es_href))
    for row in COMMON:
        body = maybe(body, *row)
    for row in m['table']:
        body = must(body, *row)
    for a in ASSETS:
        body = body.replace(f'src="{a}', f'src="../{a}').replace(f'poster="{a}', f'poster="../{a}')
    for p in ENGLISH_ONLY:
        body = body.replace(f'href="{p}', f'href="../{p}')
    # anything left in English that we know the shape of
    leftovers = re.findall(r'>(Download|Pricing|Features|Platforms|Install|Step \d|Yes|Free)<', body)
    assert not leftovers, leftovers
    return head + style + body


def main():
    (ROOT / 'es').mkdir(exist_ok=True)
    for page in PAGES:
        src = ROOT / page
        en = prepare_english(page, src.read_text())
        src.write_text(en)
        (ROOT / 'es' / page).write_text(translate(page, en))
        print('wrote site/es/' + page)


if __name__ == '__main__':
    main()
