"""Three small numpy networks, one per head, with the backward pass written out.

Each net is `X -> 48 (relu) -> 16 (relu) -> 1`, trained with Adam on minibatches of
256 and stopped when the tune loss has not improved for 10 epochs, keeping the best
weights seen. The success head's output is a logit trained with binary cross entropy;
the millis head fits `log1p(millis)` with a Huber loss, so one slow outlier does not
steer it; the credits head fits `log1p(credits)` with squared error.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

HIDDEN = (48, 16)
LEARNING_RATE = 1e-3
BATCH = 256
MAX_EPOCHS = 200
PATIENCE = 10
HUBER_DELTA = 1.0

RELU = 1
IDENTITY = 0


def loss_and_dz(kind: str, z, y, w):
    """The weighted mean loss of one head's outputs and its gradient in `z`."""
    total = w.sum()
    if kind == "bce":
        # log(1 + exp(z)) - y z, written so a large |z| does not overflow.
        e = np.exp(-np.abs(z))
        per = np.maximum(z, 0) - y * z + np.log1p(e)
        dz = np.where(z >= 0, 1.0 / (1.0 + e), e / (1.0 + e)) - y
    elif kind == "huber":
        r = z - y
        a = np.abs(r)
        per = np.where(a <= HUBER_DELTA, 0.5 * r * r, HUBER_DELTA * (a - 0.5 * HUBER_DELTA))
        dz = np.clip(r, -HUBER_DELTA, HUBER_DELTA)
    elif kind == "l2":
        r = z - y
        per = 0.5 * r * r
        dz = r
    else:
        raise ValueError(kind)
    return float((w * per).sum() / total), w * dz / total


@dataclass
class Net:
    """One head: a list of (W, b) with relu between layers and none on the last."""

    kind: str
    weights: list[np.ndarray] = field(default_factory=list)
    biases: list[np.ndarray] = field(default_factory=list)

    @classmethod
    def init(cls, kind: str, sizes: list[int], rng: np.random.Generator, out_bias: float = 0.0):
        net = cls(kind)
        for fan_in, fan_out in zip(sizes[:-1], sizes[1:], strict=True):
            net.weights.append(rng.normal(0.0, np.sqrt(2.0 / fan_in), (fan_out, fan_in)))
            net.biases.append(np.zeros(fan_out))
        net.biases[-1][:] = out_bias
        return net

    def forward(self, X):
        activations = [X]
        h = X
        last = len(self.weights) - 1
        for i, (W, b) in enumerate(zip(self.weights, self.biases, strict=True)):
            h = h @ W.T + b
            if i < last:
                h = np.maximum(h, 0.0)
            activations.append(h)
        return h[:, 0], activations

    def backward(self, activations, dz):
        grads_w = [None] * len(self.weights)
        grads_b = [None] * len(self.weights)
        delta = dz[:, None]
        for i in range(len(self.weights) - 1, -1, -1):
            grads_w[i] = delta.T @ activations[i]
            grads_b[i] = delta.sum(axis=0)
            if i > 0:
                delta = (delta @ self.weights[i]) * (activations[i] > 0)
        return grads_w, grads_b

    def loss_and_grads(self, X, y, w):
        z, activations = self.forward(X)
        loss, dz = loss_and_dz(self.kind, z, y, w)
        return loss, *self.backward(activations, dz)

    def params(self) -> list[np.ndarray]:
        return [*self.weights, *self.biases]

    def copy(self) -> Net:
        return Net(self.kind, [w.copy() for w in self.weights], [b.copy() for b in self.biases])

    def layers(self) -> list[tuple[np.ndarray, np.ndarray, int]]:
        last = len(self.weights) - 1
        return [
            (W, b, IDENTITY if i == last else RELU)
            for i, (W, b) in enumerate(zip(self.weights, self.biases, strict=True))
        ]


def fit_net(kind, X, y, w, X_tune, y_tune, w_tune, seed, max_epochs=MAX_EPOCHS,
            patience=PATIENCE) -> tuple[Net, dict]:
    rng = np.random.default_rng(seed)
    out_bias = 0.0
    if kind != "bce":
        out_bias = float(np.average(y, weights=w))
    net = Net.init(kind, [X.shape[1], *HIDDEN, 1], rng, out_bias)
    params = net.params()
    m = [np.zeros_like(p) for p in params]
    v = [np.zeros_like(p) for p in params]
    beta1, beta2, eps = 0.9, 0.999, 1e-8
    step = 0
    best = (np.inf, net.copy(), 0)
    history = []
    for epoch in range(max_epochs):
        order = rng.permutation(len(X))
        for start in range(0, len(X), BATCH):
            idx = order[start : start + BATCH]
            _, gw, gb = net.loss_and_grads(X[idx], y[idx], w[idx])
            step += 1
            for p, g, mi, vi in zip(net.params(), [*gw, *gb], m, v, strict=True):
                mi *= beta1
                mi += (1 - beta1) * g
                vi *= beta2
                vi += (1 - beta2) * g * g
                m_hat = mi / (1 - beta1**step)
                v_hat = vi / (1 - beta2**step)
                p -= LEARNING_RATE * m_hat / (np.sqrt(v_hat) + eps)
        z, _ = net.forward(X_tune)
        tune_loss, _ = loss_and_dz(kind, z, y_tune, w_tune)
        history.append(tune_loss)
        if tune_loss < best[0] - 1e-6:
            best = (tune_loss, net.copy(), epoch)
        elif epoch - best[2] >= patience:
            break
    return best[1], {"epochs": len(history), "best_epoch": best[2], "tune_loss": best[0]}


@dataclass
class MlpModel:
    heads: list[Net]
    info: dict = field(default_factory=dict)
    kind: str = "mlp"

    @classmethod
    def fit(cls, X, targets, weights, X_tune, targets_tune, weights_tune, seed: int,
            max_epochs: int = MAX_EPOCHS) -> MlpModel:
        X = X.astype(np.float64)
        X_tune = X_tune.astype(np.float64)
        heads, info = [], {}
        for at, (name, kind) in enumerate(zip(("success", "millis", "credits"),
                                              ("bce", "huber", "l2"), strict=True)):
            net, stats = fit_net(kind, X, targets[at], weights[at], X_tune, targets_tune[at],
                                 weights_tune[at], seed + at, max_epochs=max_epochs)
            heads.append(net)
            info[name] = stats
        return cls(heads, info)

    def predict(self, X):
        """(success logit, log1p millis, log1p credits)."""
        X = np.asarray(X, dtype=np.float64)
        return tuple(net.forward(X)[0] for net in self.heads)

    def to_tables(self) -> list[list[tuple[np.ndarray, np.ndarray, int]]]:
        return [net.layers() for net in self.heads]

    def save(self, path) -> None:
        arrays = {}
        for h, net in enumerate(self.heads):
            arrays[f"kind{h}"] = np.array(net.kind)
            for i, (W, b) in enumerate(zip(net.weights, net.biases, strict=True)):
                arrays[f"w{h}_{i}"] = W
                arrays[f"b{h}_{i}"] = b
        np.savez(path, **arrays)

    @classmethod
    def load(cls, path) -> MlpModel:
        data = np.load(path)
        heads = []
        for h in range(3):
            net = Net(str(data[f"kind{h}"]))
            i = 0
            while f"w{h}_{i}" in data:
                net.weights.append(data[f"w{h}_{i}"])
                net.biases.append(data[f"b{h}_{i}"])
                i += 1
            heads.append(net)
        return cls(heads)
