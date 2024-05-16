# Running attestation service:
/etc/systemd/system/attestation.service

```
[Unit]
Description=Attestation service

[Service]
Type=simple
ExecStart=/usr/bin/erc20_processor run --keep-running --http --http-addr 0.0.0.0 --http-port 15555
WorkingDirectory=/erc20lib

[Install]
WantedBy=default.target
```