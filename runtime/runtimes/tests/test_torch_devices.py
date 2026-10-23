"""Tests for where the torch sidecars run: Metal or CUDA when the card can
run what torch was built for, else the CPU, and the image sidecar's
precision and generator on each. None of these load a model."""

import sys
from types import SimpleNamespace

import pytest


def fake_torch(mps=False, cuda=False, capability=(8, 6), archs=("sm_75", "sm_80", "sm_90a")):
    return SimpleNamespace(
        float16="float16",
        float32="float32",
        backends=SimpleNamespace(mps=SimpleNamespace(is_available=lambda: mps)),
        cuda=SimpleNamespace(
            is_available=lambda: cuda,
            get_device_capability=lambda: capability,
            get_arch_list=lambda: list(archs),
        ),
        Generator=Generator,
    )


class Generator:
    def __init__(self, device):
        self.device = device
        self.seed = None

    def manual_seed(self, seed):
        self.seed = seed
        return self


@pytest.mark.parametrize("sidecar", ["audio_gen", "diffusers_sidecar", "laya", "zerank"])
def test_cuda_runs_only_on_a_card_torch_was_built_for(sidecar, request):
    module = request.getfixturevalue(sidecar)
    assert module.cuda_runs(fake_torch(cuda=True, capability=(8, 6)))
    assert module.cuda_runs(fake_torch(cuda=True, capability=(7, 5)))
    assert module.cuda_runs(fake_torch(cuda=True, capability=(12, 0)))
    assert not module.cuda_runs(fake_torch(cuda=True, capability=(6, 1)))
    assert not module.cuda_runs(fake_torch(cuda=True, capability=(7, 5), archs=("sm_80", "sm_90")))
    assert module.cuda_runs(fake_torch(cuda=True, capability=(5, 0), archs=("sm_50", "compute_50")))
    assert not module.cuda_runs(fake_torch(cuda=True, archs=()))
    assert not module.cuda_runs(fake_torch(cuda=False))


@pytest.mark.parametrize("sidecar", ["audio_gen", "diffusers_sidecar"])
def test_a_pipeline_runs_on_metal_then_a_card_it_can_use_then_the_cpu(sidecar, request):
    module = request.getfixturevalue(sidecar)
    assert module.device_of(fake_torch(mps=True)) == "mps"
    assert module.device_of(fake_torch(cuda=True)) == "cuda"
    assert module.device_of(fake_torch(cuda=True, capability=(6, 1))) == "cpu"
    assert module.device_of(fake_torch()) == "cpu"


class Pipeline:
    loads: list[tuple[str, str | None]] = []
    failures = 0

    def __init__(self, dtype):
        self.dtype = dtype
        self.device = None

    @classmethod
    def from_pretrained(cls, path, torch_dtype, local_files_only, variant):
        cls.loads.append((torch_dtype, variant))
        if len(cls.loads) <= cls.failures:
            raise OSError("no such variant")
        return cls(torch_dtype)

    def to(self, device):
        self.device = device
        return self

    def set_progress_bar_config(self, disable):
        pass


@pytest.mark.parametrize(
    ("device", "half"), [("mps", "float16"), ("cuda", "float16"), ("cpu", "float32")]
)
def test_a_pipeline_loads_in_half_precision_only_on_a_gpu(
    diffusers_sidecar, tmp_path, device, half
):
    for failures, loads in [
        (0, [(half, None)]),
        (1, [(half, None), (half, "fp16")]),
        (2, [(half, None), (half, "fp16"), ("float32", None)]),
    ]:
        Pipeline.loads, Pipeline.failures = [], failures
        pipe = diffusers_sidecar.load_pipeline(str(tmp_path), fake_torch(), Pipeline, device)
        assert Pipeline.loads == loads
        assert pipe.device == device
        assert pipe.dtype == loads[-1][0]


@pytest.mark.parametrize("device", ["mps", "cuda", "cpu"])
def test_the_seed_s_generator_lives_on_the_pipeline_s_device(diffusers_sidecar, device):
    made = diffusers_sidecar.generator(fake_torch(), device, 7)
    assert (made.device, made.seed) == (device, 7)


@pytest.mark.parametrize("sidecar", ["laya", "zerank"])
def test_a_judge_runs_on_a_card_it_can_use_then_metal_then_the_cpu(sidecar, request, monkeypatch):
    module = request.getfixturevalue(sidecar)
    for torch, device in [
        (fake_torch(cuda=True), "cuda"),
        (fake_torch(cuda=True, capability=(6, 1)), "cpu"),
        (fake_torch(cuda=True, capability=(6, 1), mps=True), "mps"),
        (fake_torch(mps=True), "mps"),
        (fake_torch(), "cpu"),
    ]:
        monkeypatch.setitem(sys.modules, "torch", torch)
        assert module.pick_device() == device
