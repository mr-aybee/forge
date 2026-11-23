<?php
// Mock portal (PHP): one HTML page saying which node and IP served it.
// Run by Forge as a service: `php -S 0.0.0.0:$PORT -t /srv/forge-portals/php`.
// `GET /?format=json` answers JSON for test scripts.

$node = gethostname();
$addresses = preg_split('/\s+/', trim((string) shell_exec('hostname -I 2>/dev/null')), -1, PREG_SPLIT_NO_EMPTY);
$info = [
    'portal' => 'php',
    'runtime' => 'PHP ' . PHP_VERSION,
    'node' => $node,
    'addresses' => $addresses,
    'port' => (int) ($_SERVER['SERVER_PORT'] ?? 0),
    'pid' => getmypid(),
    'host_header' => $_SERVER['HTTP_HOST'] ?? '',
    'client' => $_SERVER['REMOTE_ADDR'] ?? '',
    'x_forwarded_for' => $_SERVER['HTTP_X_FORWARDED_FOR'] ?? '',
    'x_real_ip' => $_SERVER['HTTP_X_REAL_IP'] ?? '',
    'scheme' => $_SERVER['HTTP_X_FORWARDED_PROTO'] ?? 'http',
    'time' => gmdate('c'),
];
header('X-Served-By: ' . $node);
error_log(($info['client']) . ' GET ' . ($_SERVER['REQUEST_URI'] ?? '/') . ' -> 200');

if (($_GET['format'] ?? '') === 'json') {
    header('Content-Type: application/json');
    echo json_encode($info);
    return;
}
$h = fn ($v) => htmlspecialchars(is_array($v) ? implode(', ', $v) : (string) $v);
?><!doctype html>
<html><head><meta charset="utf-8"><title>PHP portal · <?= $h($node) ?></title>
<meta name="forge-node" content="<?= $h($node) ?>"><meta name="viewport" content="width=device-width">
<style>body{font:15px system-ui;margin:2rem;background:#1a1330;color:#ece6f7}h1{color:#a78bfa}
table{border-collapse:collapse}th,td{padding:.35rem .8rem;border-bottom:1px solid #3b2e5c;text-align:left}th{color:#b3a4d6;font-weight:500}</style>
</head><body><h1>PHP portal</h1><p>Served by <b><?= $h($node) ?></b> (<?= $h($addresses) ?>)</p>
<table><?php foreach ($info as $k => $v): ?><tr><th><?= $h($k) ?></th><td><?= $h($v) ?></td></tr><?php endforeach ?></table>
</body></html>
