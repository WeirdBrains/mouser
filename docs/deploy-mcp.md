# Deploying the MCP server

Mouser's MCP server ([MCP](./mcp.md)) runs inside the Mouser window you have open;
there is no separate daemon. For clients on the same machine nothing needs
deploying: loopback HTTP or `mouser mcp serve --stdio`.

To reach it from other machines:

1. Turn on HTTPS (`mouser mcp enable --https`, or `[mcp.server] https = true`); plain HTTP never leaves 127.0.0.1.
2. Set `bind` to the address to listen on. Off-loopback is HTTPS with the token only.
3. Pick a certificate: your own (`cert` + `key`), the generated self-signed one
   (clients must trust it), or Let's Encrypt (`[mcp.server.tls] source = "acme"`;
   see [MCP §Let's Encrypt](./mcp.md#lets-encrypt)). tls-alpn-01 needs a public DNS name
   and port 443 forwarded to `https_port`, http-01 a port-80 listener, dns-01 a hook or
   a TXT record set by hand.
4. Check it with `mouser mcp status` and `mouser mcp cert status`; `mouser mcp cert renew` renews now.
   Certificates renew 30 days before expiry without a restart.
5. Give each client the token (`mouser mcp token --show`; `--rotate` replaces it) and keep it secret; the
   permissions in [MCP §Security](./mcp.md#security) still apply per tool.
