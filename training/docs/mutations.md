# Guards broken by hand

Each guard below was broken in place. The named tests were run, and the change was
reverted. If breaking a guard had turned nothing red, that guard would be a check that
cannot fail. Every guard here turned at least one test red.

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
| A residential effect that flips inside the test window is rejected | `synth._scenario_planted` keeps `success_when_flipped` at 0.90 in the reversal scenario, so nothing flips | `test_reversal_in_the_test_window_is_rejected` (`gates` exits 0) |
| The stable control needs overrides applied | `thresholds.applies` is ANDed with `False`, so the policy abstains on every row | `test_stable_control_passes_with_overrides_applied` (coverage 0.0 fails first) |
| A floors file's `min_coverage` is checked | `floors.comparison_checks` skips the coverage check | `test_abstaining_everything_does_not_pass_the_control` (no coverage line is printed) |
| The tradeoff sweep chooses floors the way `thresholds` does | `tradeoff.sweep` passes `min_covered` 0 to `thresholds.choose` | `test_tradeoff_coverage_is_monotone_in_r_max` (1335 overrides against the run's 1059) |
| Nothing before the gate reads the test window | `models.train` chooses tau from every row again | `test_the_test_window_is_never_read_before_the_gate` (tau 0.8657 against 0.8685 on the rewritten window) |
| A broken content override is harmful | `compare.from_outcomes` counts only a lost success as harmful | `test_comparison_counts_every_pair_the_way_it_was_built` |

Two notes from the runs. With the printable run guard removed, the artifacts that the
synth models export fail the size and run test. The run breaker in `export.serialize`
therefore does real work on trained weights, not only on the planted blob. With the CRC
check removed, `test_the_reader_refuses_bad_headers_and_truncation` still passes. The
reader's length checks catch truncation on their own. The CRC test is the one that
catches a flipped byte.
