# winter-client (Python)

Emit rich [Terminal Block Protocol](../../docs/terminal-block-protocol-spec.md)
(TBP) blocks from Python. Falls back to `text/plain` when Winter is not the active
terminal, so scripts stay safe under tmux, ssh, and CI.

```python
import winter

winter.display(dataframe)                  # uses the object's _repr_*_ methods
winter.display_svg(open("plot.svg").read())
winter.display_image("chart.png")
winter.display_markdown("# hello")
```

## Develop

```bash
cd clients/client-py
python -m pytest          # tests (pythonpath=src is configured)
ruff check .
```
