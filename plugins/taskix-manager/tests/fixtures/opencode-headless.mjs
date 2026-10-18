import { delimiter, dirname, join } from "node:path";

/** Retain executable/network plumbing, never host provider tokens or user config. */
export function isolatedOpenCodeEnvironment(
    home,
    config,
    taskix,
    inherited = process.env,
) {
    const env = {};
    for (const key of [
        "PATH",
        "Path",
        "SystemRoot",
        "SYSTEMROOT",
        "ComSpec",
        "COMSPEC",
        "PATHEXT",
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
        "NODE_EXTRA_CA_CERTS",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ])
        if (inherited[key]) env[key] = inherited[key];
    env.PATH = [
        taskix && dirname(taskix),
        dirname(process.execPath),
        env.PATH || env.Path,
    ]
        .filter(Boolean)
        .join(delimiter);
    delete env.Path;
    Object.assign(env, {
        TMPDIR: join(home,"tmp"), TEMP:join(home,"tmp"), TMP:join(home,"tmp"),
        HOME: home,
        USERPROFILE: home,
        APPDATA: join(home, "appdata"),
        LOCALAPPDATA: join(home, "localappdata"),
        XDG_CONFIG_HOME: join(home, "config"),
        XDG_DATA_HOME: join(home, "data"),
        XDG_STATE_HOME: join(home, "state"),
        XDG_CACHE_HOME: join(home, "cache"),
        OPENCODE_CONFIG_DIR: config,
        OPENCODE_DISABLE_DEFAULT_PLUGINS: "true",
        OPENCODE_DISABLE_MODELS_FETCH: "true",
        OPENCODE_DISABLE_AUTOUPDATE: "true",
        TASKIX_JEV_ENABLED: "false",
    });
    if (inherited.AGENTIX_TEST_NPM_CACHE)
        env.npm_config_cache = inherited.AGENTIX_TEST_NPM_CACHE;
    return env;
}
