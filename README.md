# caching-http-server

Simple caching http server for development

```text
Usage: caching-http-server [OPTIONS]

Options:
  -b, --bind <BIND>       [default: localhost:7776]
  -d, --database <FILE>   [default: :memory:]
  -t, --ttl <SECONDS>     [default: 0]
      --no-client-errors
      --server-errors
  -h, --help              Print help information

```

Then you can point your HTTP client to
<http://localhost:7776/http://example.com/> or whichever (http or
https) URL you want to visit.
