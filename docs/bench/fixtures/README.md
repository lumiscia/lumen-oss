# Benchmark Fixtures

Canonical fixture set shared by render backend benchmarks and parity checks.

## Files

- `vector-heavy.json`
- `effects-heavy.json`
- `mixed-media.json`
- `text-heavy.json`

## Rules

1. These fixtures are the single source for backend comparisons.
2. Backend benchmarks must not modify fixture JSON at runtime.
3. Any fixture change requires:
   - baseline report refresh,
   - comparison report note,
   - parity tolerance review.
