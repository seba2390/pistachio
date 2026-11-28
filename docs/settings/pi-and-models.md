# pi & models

Which pi Pistachio runs, the model new sessions start with, and the models you have connected.

## pi

```{image} /_static/screenshots/pi-pi-light.webp
:class: shot only-light
:alt: The pi section
```

```{image} /_static/screenshots/pi-pi-dark.webp
:class: shot only-dark
:alt: The pi section
```

pi program
: The `pi` to run. Leave it empty to find pi on your `PATH`. The page shows the version it
  found. Used by new sessions. If `PISTACHIO_PI_BIN` is set, it takes precedence. When pi is not
  on the `PATH`, Pistachio fills this in with the one it finds, installs or is shown at startup
  (see [Finding pi](../general.md#finding-pi)).

## Startup model

```{image} /_static/screenshots/pi-startup-light.webp
:class: shot only-light
:alt: The Startup model section
```

```{image} /_static/screenshots/pi-startup-dark.webp
:class: shot only-dark
:alt: The Startup model section
```

The model and thinking level new sessions start with, or **Let pi choose**. They are saved in
pi's own `settings.json`, like `/settings` in the terminal, so the terminal pi uses them too.

## Connected models

```{image} /_static/screenshots/pi-connected-light.webp
:class: shot only-light
:alt: Connected models
```

```{image} /_static/screenshots/pi-connected-dark.webp
:class: shot only-dark
:alt: Connected models
```

The servers and API keys in `models.json` on this computer, grouped by where the models run:
**This device**, **Other devices** and **API connections**. Each shows whether its server
answers; click one to change it. The trash button removes it after asking: an API connection or
a server on another device is only removed from `models.json`, and so is a server on this device
that Pistachio did not set up (like Ollama). A model Pistachio downloaded and runs on this device
is also stopped and deleted, with its engine unless another local model uses it. Such a model
also has a play / stop button: play starts its engine and loads the model (the row says
**Starting…** until it answers), stop ends it; its files stay. Sign-ins made
with `/login` are not listed here.

**Add models…** (also **Add or change models…** in the model menu of the message box) starts
with two choices:

Locally hosted model
: Choose whether it runs on this device or another one (by its address, like
  `192.168.1.89`). On this device (when pi runs here) there are two ways:
  **Attach to existing model** and **Deploy new model**. Attaching (and another device) looks
  for Ollama, LM Studio, llama.cpp, MLX, vLLM, Jan, Docker Model Runner, SGLang, Lemonade and
  KoboldCpp on their usual ports. Any other server is connected by its port or address;
  Pistachio finds out whether it speaks OpenAI's, Anthropic's or Gemini's API.

  Each choice opens a page of its own, and the back arrow returns to the one before.

  ```{image} /_static/screenshots/models-ways-light.webp
  :class: shot only-light
  :alt: Attach to existing model or Deploy new model, on this device
  ```

  ```{image} /_static/screenshots/models-ways-dark.webp
  :class: shot only-dark
  :alt: Attach to existing model or Deploy new model, on this device
  ```

  **Deploy new model** suggests models for this computer. **Scan local hardware** looks at the
  processor, the memory, the graphics, the free disk and the build tools, with a progress bar
  that says what it is looking at, and lists what it found; nothing leaves the computer.
  **Suggest models** then offers up to four: **Tiny**, the most capable small model (3B
  parameters or fewer, in its lightest build) that nearly any computer runs, then **Fast**,
  **Balanced** and **Most capable**, each faster and less capable than the next. Each starts collapsed
  to its name and its **Download and run** button; the arrow along its bottom opens it. Opened,
  it says why it fits, its context, whether it takes images and whether it thinks (with the
  thinking levels pi offers for it), and shows the
  inference engine that runs it best here, the checkpoint and its quantization, the memory it
  takes and its speed here (measured or estimated); the engine and the checkpoint link to their
  pages. Each comes with a
  default engine that installs without any steps (llama.cpp, Rapid-MLX on Macs, Splash on newer
  Macs, BigMoeOnEdge for models larger than the memory) and, where one is clearly faster, a
  specialized engine that is built on this computer (ik_llama.cpp, TurboFieldfare). The **Download and
  run** button says how much it is; pressing it shows exactly what is downloaded (the engine and
  the model's files, linked to their pages, with their sizes and licenses) and asks first. While
  it downloads, the card shows the step, how much has arrived, a progress bar and **Cancel**,
  which stops it at once. Without an internet connection the button says **No internet** and
  nothing is downloaded. Everything is pinned to a version and
  ```{image} /_static/screenshots/models-deploy-light.webp
  :class: shot only-light
  :alt: The hardware this device has and the four models suggested for it
  ```

  ```{image} /_static/screenshots/models-deploy-dark.webp
  :class: shot only-dark
  :alt: The hardware this device has and the four models suggested for it
  ```

  ```{image} /_static/screenshots/models-suggestion-light.webp
  :class: shot only-light
  :alt: A suggested model opened: its engine, quantization, memory, speed, context and thinking
  ```

  ```{image} /_static/screenshots/models-suggestion-dark.webp
  :class: shot only-dark
  :alt: A suggested model opened: its engine, quantization, memory, speed, context and thinking
  ```

  checked against its SHA-256, and kept in `~/.pi/agent/pistachio/local-models`. Once it runs,
  the model is connected like any other server, set up so that the thinking levels of the
  message box reach it. It runs for this computer only, is started when
  a chat sends to it, and stops when Pistachio quits.

API provider model
: pi's own providers (Anthropic, OpenAI, Google Gemini, OpenRouter and more). Paste an API key
  (or `$NAME` for an environment variable, or `!command`), which is checked with the provider
  where it allows that, or sign in. All the provider's models then appear in the model menu.

For a server, Pistachio asks which models it has and you tick the ones to add. Their context
size, longest answer, and whether they think or see pictures are filled in as far as the server
says, and can be changed. A model can be tested with a short message before it is added, and
added by its ID when the server has no list.

When a thinking model is added, Pistachio checks how its server takes thinking levels, in a few
seconds: servers differ (and so do their versions) in how they are asked to think, and a model's
template decides which efforts change anything. vLLM shows the prompt it builds for each way of
asking, llama.cpp lists the efforts its template knows, and one short reply with thinking off
shows whether off really stops it. The levels of the message box are set up from what it finds;
levels that do the same thing show as one. A model that thinks a little at any effort, or can't
stop, shows **lowest** instead of **off**.

**Edit models.json…** opens the file as text, with validation. The previous version is kept as
`models.json.bak-pistachio`.

pi reads `models.json` only when it starts, so after a change it restarts on the same session
(after the current turn) and selects the new model. In a session on an SSH host, the models are
still the ones on this computer (see [SSH connections](ssh-connections.md#models-on-ssh-hosts)).
