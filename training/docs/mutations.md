# Guards broken by hand

Each guard below was broken in place, the named tests were run, and the change was
reverted. A guard whose break turned nothing red would have been a check that cannot
fail; none of these did.

| guard | how it was broken | what went red |
|---|---|---|
| The threshold sweep abstains when no floor meets the bound | `thresholds.sweep` starts from `t[-1]` instead of NaN, so a code always gets a floor | `test_a_sweep_with_no_safe_floor_abstains`, `test_a_row_that_is_not_cheaper_is_never_covered` |
| Bootstrap resamples pairs, not rows | `bootstrap_pair_weights` takes `np.arange(len(pairs))` as the unit | `test_bootstrap_resamples_whole_pairs`, `test_clustered_regressions_widen_the_bound` |
| The chronological split cuts whole days, pairs intact | `splits.chronological` assigns each row by its index in the list | `test_pairs_never_split_chronologically`, `test_chronological_windows_follow_day_order_in_whole_days` |
| Tau comes from the repeat pairs | `labels.choose_tau` returns 0 | `test_choose_tau_lands_on_the_planted_repeat_percentile` |
| Class weights balance success and failure | `models.class_weights` returns `(1.0, 1.0)` | `test_class_weights_balance_the_classes` |
| A host-like string in a row is refused | the `HOST_LIKE` search is removed from `dataset.validate_rows`, leaving only `://` | `test_a_host_like_string_is_named` |
| No printable run over 8 bytes after the header | the length test is removed from `export.violations` and from `export.audit` | `test_export_has_no_ascii_run_and_fits_size[mlp]`, `[gbdt]`, `test_a_planted_ascii_run_or_host_is_broken_and_audited` |
| Fewer than 300 pairs is insufficient | `gates.MIN_PAIRS = 0` | `test_gates_report_insufficient_below_min_pairs` |
| An unsupported cell is never applied | `thresholds.applies` ORs the support mask with `True` | `test_an_unsupported_cell_is_never_applied` |
| A CRC mismatch is refused | the CRC comparison in `predict.read` is short-circuited with `False and` | `test_crc_mismatch_is_refused` |

Two notes from the runs. With the printable run guard removed, the artifacts the synth
models export fail the size and run test, so the run breaker in `export.serialize` is
doing real work on trained weights, not only on the planted blob. With the CRC check
removed, `test_the_reader_refuses_bad_headers_and_truncation` still passes, because
the reader's length checks catch truncation on their own; the CRC test is the one that
holds a flipped byte.
