"""Three gradient boosted tree heads on the same matrix as the MLP.

After fitting, `to_tables` flattens each booster into the artifact's node layout:
per head a list of trees, each a list of `(feature, default_left, threshold, left,
right, value)` with node 0 the root, a leaf marked by `left == right == 0xFFFF`, and
`x <= threshold` going left, which is LightGBM's own rule. LightGBM folds its starting
score into the first tree's leaves, so the base score written is zero and the sum of
the leaves is the raw score.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import lightgbm as lgb
import numpy as np

LEAF = 0xFFFF
MAX_ROUNDS = 300
EARLY_STOP = 30

PARAMS = {
    "num_leaves": 15,
    "max_depth": 5,
    "min_data_in_leaf": 50,
    "feature_fraction": 0.8,
    "learning_rate": 0.05,
    "verbosity": -1,
    "deterministic": True,
    "force_row_wise": True,
    "num_threads": 1,
}

OBJECTIVES = (
    {"objective": "binary"},
    {"objective": "huber", "alpha": 1.0},
    {"objective": "regression"},
)

Node = tuple[int, bool, float, int, int, float]


def _flatten(tree: dict) -> list[Node]:
    """One dumped tree as nodes in breadth-first order, root first."""
    nodes: list[Node] = []
    queue = [tree["tree_structure"]]
    slots: list[dict] = []
    while queue:
        node = queue.pop(0)
        slots.append(node)
        if "split_index" in node:
            queue.append(node["left_child"])
            queue.append(node["right_child"])
    index = {id(node): at for at, node in enumerate(slots)}
    for node in slots:
        if "split_index" in node:
            if node.get("decision_type", "<=") != "<=":
                raise ValueError("only numeric <= splits can be exported")
            nodes.append((
                int(node["split_feature"]),
                bool(node.get("default_left", True)),
                float(node["threshold"]),
                index[id(node["left_child"])],
                index[id(node["right_child"])],
                0.0,
            ))
        else:
            nodes.append((0, False, 0.0, LEAF, LEAF, float(node["leaf_value"])))
    return nodes


def eval_tables(trees: list[list[Node]], base_score: float, X) -> np.ndarray:
    """The raw score of flattened trees, with the artifact's rules, in float64."""
    X = np.asarray(X, dtype=np.float64)
    out = np.full(len(X), base_score, dtype=np.float64)
    for nodes in trees:
        for r in range(len(X)):
            at = 0
            while True:
                feature, default_left, threshold, left, right, value = nodes[at]
                if left == LEAF and right == LEAF:
                    out[r] += value
                    break
                x = X[r, feature]
                go_left = default_left if not np.isfinite(x) else x <= threshold
                at = left if go_left else right
    return out


@dataclass
class GbdtModel:
    boosters: list[lgb.Booster]
    tables: list[list[list[Node]]] = field(default_factory=list)
    info: dict = field(default_factory=dict)
    kind: str = "gbdt"

    @classmethod
    def fit(cls, X, targets, weights, X_tune, targets_tune, weights_tune, seed: int,
            max_rounds: int = MAX_ROUNDS) -> GbdtModel:
        boosters, info = [], {}
        for at, name in enumerate(("success", "millis", "credits")):
            params = {**PARAMS, **OBJECTIVES[at], "seed": seed + at}
            train = lgb.Dataset(X, label=targets[at], weight=weights[at], free_raw_data=False)
            tune = lgb.Dataset(X_tune, label=targets_tune[at], weight=weights_tune[at],
                               reference=train)
            booster = lgb.train(
                params,
                train,
                num_boost_round=max_rounds,
                valid_sets=[tune],
                callbacks=[lgb.early_stopping(EARLY_STOP, verbose=False)],
            )
            boosters.append(booster)
            info[name] = {"trees": booster.best_iteration or booster.num_trees()}
        model = cls(boosters, info=info)
        model.tables = model._dump()
        return model

    def _dump(self) -> list[list[list[Node]]]:
        out = []
        for booster in self.boosters:
            best = booster.best_iteration or None
            dump = booster.dump_model(num_iteration=best)
            out.append([_flatten(tree) for tree in dump["tree_info"]])
        return out

    def to_tables(self) -> list[list[list[Node]]]:
        return self.tables

    def predict(self, X):
        """(success logit, log1p millis, log1p credits), from the flattened tables so
        a dropped tree counts."""
        return tuple(eval_fast(trees, X) for trees in self.tables)

    def cap_bytes(self, limit: int = 1_500_000, size_of=None) -> int:
        """Drop the last tree of each head in turn until the artifact fits. Returns how
        many were dropped."""
        from ..export import artifact_size

        size_of = size_of or (lambda: artifact_size(self))
        dropped = 0
        head = 0
        while size_of() > limit:
            if all(len(trees) <= 1 for trees in self.tables):
                raise ValueError("cannot fit the artifact under the cap with one tree per head")
            if len(self.tables[head]) > 1:
                self.tables[head].pop()
                dropped += 1
            head = (head + 1) % len(self.tables)
        return dropped

    def save(self, directory) -> None:
        directory = Path(directory)
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "tables.json").write_text(json.dumps(self.tables))

    @classmethod
    def load(cls, directory) -> GbdtModel:
        tables = json.loads((Path(directory) / "tables.json").read_text())
        return cls([], tables=[[[tuple(n) for n in tree] for tree in head] for head in tables])


def eval_fast(trees: list[list[Node]], X) -> np.ndarray:
    """`eval_tables`, vectorised over rows."""
    X = np.asarray(X, dtype=np.float64)
    out = np.zeros(len(X), dtype=np.float64)
    rows = np.arange(len(X))
    for nodes in trees:
        table = np.array(nodes, dtype=np.float64)
        at = np.zeros(len(X), dtype=np.int64)
        for _ in range(len(nodes)):
            left = table[at, 3]
            leaf = (left == LEAF) & (table[at, 4] == LEAF)
            if leaf.all():
                break
            x = X[rows, table[at, 0].astype(np.int64)]
            finite = np.isfinite(x)
            go_left = np.where(finite, x <= table[at, 2], table[at, 1] > 0)
            nxt = np.where(go_left, table[at, 3], table[at, 4]).astype(np.int64)
            at = np.where(leaf, at, nxt)
        out += table[at, 5]
    return out
