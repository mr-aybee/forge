// Mock portal (.NET 10 file-based app): a small multi-page app that says
// which node served it. Run by Forge as a service:
//   dotnet run /srv/forge-portals/dotnet/portal.cs
// Port from $PORT (default 8103). Pages:
//   /         home; `/?format=json` answers JSON for test scripts
//   /notes    notes stored in PostgreSQL (GET lists, POST adds)
//   /health   JSON health: process and database
//   /about    node, addresses and request headers
// Every page has a footer with the node name and sends `X-Served-By`.
//
// Optional, by environment variable:
//   DATABASE_URL   postgres://user:password@host:5432/db
//   FORGE_URL      Forge server base URL, for example http://172.17.0.1:55130
//   FORGE_LOG_KEY  log ingest key (flk_...): Serilog writes to Forge's
//                  Seq-compatible endpoint
#:sdk Microsoft.NET.Sdk.Web
#:package Npgsql@9.0.3
#:package Serilog.AspNetCore@9.0.0
#:package Serilog.Sinks.Seq@9.0.0
// File-based apps default to native AOT, which turns off reflection-based JSON.
#:property PublishAot=false

using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using Npgsql;
using Serilog;

var port = int.TryParse(Environment.GetEnvironmentVariable("PORT"), out var p) ? p : 8103;
var node = Dns.GetHostName();
var runtime = RuntimeInformation.FrameworkDescription;
var forgeUrl = (Environment.GetEnvironmentVariable("FORGE_URL") ?? "").TrimEnd('/');
var logKey = Environment.GetEnvironmentVariable("FORGE_LOG_KEY") ?? "";

var logConfig = new LoggerConfiguration()
    .MinimumLevel.Information()
    .MinimumLevel.Override("Microsoft", Serilog.Events.LogEventLevel.Warning)
    .Enrich.WithProperty("Service", "net-portal")
    .Enrich.WithProperty("Node", node)
    .WriteTo.Console();
if (forgeUrl.Length > 0 && logKey.Length > 0) logConfig = logConfig.WriteTo.Seq(forgeUrl, apiKey: logKey);
Log.Logger = logConfig.CreateLogger();

var db = Db.FromUrl(Environment.GetEnvironmentVariable("DATABASE_URL"));
var builder = WebApplication.CreateBuilder(args);
builder.WebHost.UseUrls($"http://0.0.0.0:{port}");
builder.Host.UseSerilog();
var app = builder.Build();
var hits = 0;

// Every request: one Information event with its total and database time.
app.Use(async (ctx, next) =>
{
    ctx.Response.Headers["X-Served-By"] = node;
    Db.StartRequest();
    await next();
});
app.UseSerilogRequestLogging(o =>
{
    o.MessageTemplate = "HTTP {RequestMethod} {RequestPath} responded {StatusCode} in {Elapsed:0.0} ms (database {DbMs} ms)";
    o.EnrichDiagnosticContext = (diag, ctx) =>
    {
        diag.Set("DbMs", Math.Round(Db.RequestMs, 1));
        diag.Set("Client", ctx.Request.Headers["X-Real-IP"].ToString() is { Length: > 0 } ip ? ip : ctx.Connection.RemoteIpAddress?.ToString() ?? "");
    };
});

static string[] Addresses() => NetworkInterface.GetAllNetworkInterfaces()
    .Where(i => i.OperationalStatus == OperationalStatus.Up && i.NetworkInterfaceType != NetworkInterfaceType.Loopback)
    .SelectMany(i => i.GetIPProperties().UnicastAddresses)
    .Where(a => a.Address.AddressFamily == AddressFamily.InterNetwork)
    .Select(a => a.Address.ToString())
    .ToArray();

static string H(object? v) => WebUtility.HtmlEncode(v is string[] a ? string.Join(", ", a) : v?.ToString() ?? "");

string[] pages = ["/", "/notes", "/health", "/about"];
string[] titles = ["Home", "Notes", "Health", "About"];
IResult Page(string path, string title, string body, int n)
{
    var nav = string.Concat(pages.Select((pg, i) => $"<a href=\"{pg}\" class=\"{(pg == path ? "on" : "")}\">{titles[i]}</a>"));
    var html = $$"""
        <!doctype html><html><head><meta charset="utf-8"><title>.NET portal · {{title}} · {{H(node)}}</title>
        <meta name="forge-node" content="{{H(node)}}"><meta name="viewport" content="width=device-width">
        <style>body{font:15px system-ui;margin:0;background:#101b14;color:#e4f2e8;display:flex;flex-direction:column;min-height:100vh}
        nav{display:flex;gap:1rem;padding:.8rem 2rem;background:#15261b;border-bottom:1px solid #24402e}nav a{color:#94bfa2;text-decoration:none}
        nav a.on{color:#4ade80;font-weight:600}main{flex:1;padding:1.5rem 2rem}h1{color:#4ade80;margin-top:0}
        table{border-collapse:collapse}th,td{padding:.35rem .8rem;border-bottom:1px solid #24402e;text-align:left}th{color:#94bfa2;font-weight:500}
        input,button{font:inherit;padding:.4rem .6rem;border-radius:4px;border:1px solid #24402e;background:#101b14;color:inherit}button{background:#4ade80;color:#101b14;border:0;cursor:pointer}
        footer{padding:.7rem 2rem;background:#15261b;border-top:1px solid #24402e;font-size:13px;color:#94bfa2}footer b{color:#4ade80}</style>
        </head><body><nav><b>.NET portal</b>{{nav}}</nav><main>{{body}}</main>
        <footer>Served by node <b>{{H(node)}}</b> · {{H(runtime)}} · pid {{Environment.ProcessId}} · request #{{n}}</footer></body></html>
        """;
    return Results.Content(html, "text/html; charset=utf-8");
}

Dictionary<string, object> Info(HttpContext ctx, int n) => new()
{
    ["portal"] = "dotnet",
    ["runtime"] = runtime,
    ["node"] = node,
    ["addresses"] = Addresses(),
    ["port"] = port,
    ["pid"] = Environment.ProcessId,
    ["hits"] = n,
    ["host_header"] = ctx.Request.Host.ToString(),
    ["client"] = ctx.Connection.RemoteIpAddress?.ToString() ?? "",
    ["x_forwarded_for"] = ctx.Request.Headers["X-Forwarded-For"].ToString(),
    ["x_real_ip"] = ctx.Request.Headers["X-Real-IP"].ToString(),
    ["scheme"] = ctx.Request.Headers["X-Forwarded-Proto"].ToString() is { Length: > 0 } s ? s : "http",
    ["time"] = DateTime.UtcNow.ToString("o"),
};

app.MapGet("/", async (HttpContext ctx) =>
{
    var n = Interlocked.Increment(ref hits);
    var info = Info(ctx, n);
    if (ctx.Request.Query["format"] == "json") return Results.Json(info);
    var status = await db.Status();
    return Page("/", "Home", $"""
        <h1>Hello from {H(node)}</h1>
        <p>This page was served by node <b>{H(node)}</b> ({H(info["addresses"])}). Reload it behind a load
        balancer and watch the node name in the footer change.</p><p>Database: <b>{H(status)}</b></p>
        """, n);
});

app.MapGet("/notes", async () =>
{
    var n = Interlocked.Increment(ref hits);
    if (!db.Configured) return Page("/notes", "Notes", "<h1>Notes</h1><p>Database not configured (set DATABASE_URL).</p>", n);
    var rows = string.Concat((await db.Notes()).Select(x =>
        $"<tr><td>{x.Id}</td><td>{H(x.App)}</td><td>{H(x.Node)}</td><td>{H(x.Body)}</td><td>{x.CreatedAt:yyyy-MM-dd HH:mm:ss}</td></tr>"));
    return Page("/notes", "Notes", $"""
        <h1>Notes</h1>
        <form method="post" action="/notes"><input name="body" placeholder="Write a note" required maxlength="500"> <button>Add</button></form>
        <p>Each note records the app and node that saved it.</p>
        <table><tr><th>#</th><th>App</th><th>Node</th><th>Note</th><th>Created (UTC)</th></tr>{rows}</table>
        """, n);
});

app.MapPost("/notes", async (HttpContext ctx) =>
{
    Interlocked.Increment(ref hits);
    if (!db.Configured) return Results.NotFound();
    var form = await ctx.Request.ReadFormAsync();
    var body = form["body"].ToString().Trim();
    if (body.Length > 500) body = body[..500];
    if (body.Length > 0)
    {
        var id = await db.Add(node, body);
        Log.Information("Note {NoteId} added on {Node}", id, node);
    }
    return Results.Redirect("/notes", permanent: false, preserveMethod: false);
});

app.MapGet("/health", async () =>
{
    var status = await db.Status();
    var ok = status == "ok" || !db.Configured;
    return Results.Json(new { status = ok ? "ok" : "degraded", node, database = status }, statusCode: ok ? 200 : 503);
});

app.MapGet("/about", (HttpContext ctx) =>
{
    var n = Interlocked.Increment(ref hits);
    var rows = string.Concat(Info(ctx, n).Select(kv => $"<tr><th>{H(kv.Key)}</th><td>{H(kv.Value)}</td></tr>"));
    return Page("/about", "About", $"<h1>About this node</h1><table>{rows}</table>", n);
});

Log.Information(".NET portal starting on {Node}:{Port}; database {Database}; logs to Forge {Logs}",
    node, port, db.Configured ? "configured" : "not configured", logKey.Length > 0 ? "on" : "off");
app.Run();

record Note(long Id, string App, string Node, string Body, DateTime CreatedAt);

sealed class Db
{
    readonly NpgsqlDataSource? source;
    bool ready;

    // Database time of the current request; the middleware resets it.
    static readonly AsyncLocal<System.Runtime.CompilerServices.StrongBox<double>?> spent = new();
    public static void StartRequest() => spent.Value = new(0);
    public static double RequestMs => spent.Value?.Value ?? 0;

    /// Runs one query, logs its time as an Information event and adds it to the request's total.
    static async Task<T> Timed<T>(string query, Func<Task<T>> run, Func<T, int> rows)
    {
        var sw = System.Diagnostics.Stopwatch.StartNew();
        try
        {
            var result = await run();
            sw.Stop();
            Log.Information("Database {Query} took {Elapsed} ms ({Rows} rows)", query, Math.Round(sw.Elapsed.TotalMilliseconds, 2), rows(result));
            return result;
        }
        finally
        {
            if (spent.Value is { } box) box.Value += sw.Elapsed.TotalMilliseconds;
        }
    }

    Db(NpgsqlDataSource? source) => this.source = source;

    public bool Configured => source is not null;

    // postgres://user:password@host:port/db -> an Npgsql connection string.
    public static Db FromUrl(string? url)
    {
        if (string.IsNullOrWhiteSpace(url)) return new Db(null);
        var u = new Uri(url);
        var userInfo = u.UserInfo.Split(':', 2);
        var cs = new NpgsqlConnectionStringBuilder
        {
            Host = u.Host,
            Port = u.Port > 0 ? u.Port : 5432,
            Username = Uri.UnescapeDataString(userInfo[0]),
            Password = userInfo.Length > 1 ? Uri.UnescapeDataString(userInfo[1]) : null,
            Database = u.AbsolutePath.TrimStart('/'),
            Timeout = 3,
        };
        return new Db(NpgsqlDataSource.Create(cs.ConnectionString));
    }

    async Task<NpgsqlConnection> Open()
    {
        var conn = await source!.OpenConnectionAsync();
        if (!ready)
        {
            await using var cmd = new NpgsqlCommand("""
                CREATE TABLE IF NOT EXISTS notes (
                  id BIGSERIAL PRIMARY KEY, app TEXT NOT NULL, node TEXT NOT NULL,
                  body TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now())
                """, conn);
            await cmd.ExecuteNonQueryAsync();
            ready = true;
        }
        return conn;
    }

    public async Task<string> Status()
    {
        if (source is null) return "not configured (set DATABASE_URL)";
        try
        {
            await Timed("ping", async () =>
            {
                await using var conn = await Open();
                await using var cmd = new NpgsqlCommand("SELECT 1", conn);
                return await cmd.ExecuteScalarAsync();
            }, _ => 1);
            return "ok";
        }
        catch (Exception e)
        {
            Log.Warning("Database ping failed: {Error}", e.Message);
            return "unreachable";
        }
    }

    public Task<List<Note>> Notes() => Timed("list notes", async () =>
    {
        await using var conn = await Open();
        await using var cmd = new NpgsqlCommand("SELECT id, app, node, body, created_at FROM notes ORDER BY id DESC LIMIT 50", conn);
        await using var r = await cmd.ExecuteReaderAsync();
        var list = new List<Note>();
        while (await r.ReadAsync()) list.Add(new Note(r.GetInt64(0), r.GetString(1), r.GetString(2), r.GetString(3), r.GetDateTime(4)));
        return list;
    }, l => l.Count);

    public Task<long> Add(string node, string body) => Timed("insert note", async () =>
    {
        await using var conn = await Open();
        await using var cmd = new NpgsqlCommand("INSERT INTO notes (app, node, body) VALUES ('dotnet', $1, $2) RETURNING id", conn);
        cmd.Parameters.AddWithValue(node);
        cmd.Parameters.AddWithValue(body);
        return (long)(await cmd.ExecuteScalarAsync())!;
    }, _ => 1);
}
