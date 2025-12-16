<p align="center"><code>npm i -g codex-rh</code><br />

<p align="center"><strong>Codex-rh</strong> is an OpenAI Codex CLI fork that runs locally on your computer, introducing plan-mode and other features like async subagents.
</br>

<p align="center">
  <img src="./.github/codex-cli-plan-mode.png" alt="Codex CLI plan mode" width="80%" />
  </p>

---

## Quickstart

### Installing and running Codex CLI

Install globally with your preferred package manager. If you use npm:

```shell
npm install -g codex-rh
```

Then run:

```shell
codex-rh
```

If you see `codex-rh: command not found`, ensure your npm global bin dir is on `PATH`:

```shell
export PATH="$(npm config get prefix)/bin:$PATH"
```

---

## License

This repository is licensed under the [Apache-2.0 License](LICENSE).
