"""Tests for where the torch sidecars run: Metal or CUDA when the card can
run what torch was built for, else the CPU, and the image sidecar's
precision and generator on each. None of these load a model."""

import sys
from types import SimpleNamespace

import pytest


def fake_torch(mps=False, cuda=False, kernel_error=None, fail_at="item"):
    probes = []

    def read():
        if kernel_error is not None:
            raise RuntimeError(kernel_error)
        return 1.0

    def ones(count, *, device):
        probes.append((count, device))
        if kernel_error is not None and fail_at == "ones":
            raise RuntimeError(kernel_error)
        return SimpleNamespace(sum=lambda: SimpleNamespace(item=read))

    return SimpleNamespace(
        float16="float16",
        float32="float32",
        backends=SimpleNamespace(mps=SimpleNamespace(is_available=lambda: mps)),
        cuda=SimpleNamespace(
            is_available=lambda: cuda,
            get_device_capability=lambda: (8, 7),
            get_arch_list=lambda: ["sm_75", "sm_80", "sm_86", "sm_90"],
        ),
        ones=ones,
        probes=probes,
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
def test_cuda_runs_only_after_a_kernel_completes(sidecar, request):
    module = request.getfixturevalue(sidecar)
    torch = fake_torch(cuda=True)
    assert module.cuda_runs(torch)
    assert torch.probes == [(1, "cuda")]
    unavailable = fake_torch(cuda=False)
    assert not module.cuda_runs(unavailable)
    assert unavailable.probes == []


@pytest.mark.parametrize("sidecar", ["audio_gen", "diffusers_sidecar", "laya", "zerank"])
@pytest.mark.parametrize("fail_at", ["ones", "item"])
@pytest.mark.parametrize(
    "error", ["no kernel image is available for execution on the device", "CUDA out of memory"]
)
def test_cuda_falls_back_when_a_kernel_cannot_run(sidecar, request, fail_at, error):
    module = request.getfixturevalue(sidecar)
    assert not module.cuda_runs(fake_torch(cuda=True, kernel_error=error, fail_at=fail_at))


@pytest.mark.parametrize("sidecar", ["audio_gen", "diffusers_sidecar", "laya", "zerank"])
def test_cuda_keeps_working_forward_compatibility(sidecar, request):
    module = request.getfixturevalue(sidecar)
    torch = fake_torch(cuda=True)
    torch.cuda.get_device_capability = lambda: (12, 0)
    torch.cuda.get_arch_list = lambda: ["sm_90", "compute_90"]
    assert module.cuda_runs(torch)


@pytest.mark.parametrize("sidecar", ["audio_gen", "diffusers_sidecar"])
def test_a_pipeline_runs_on_metal_then_a_card_it_can_use_then_the_cpu(sidecar, request):
    module = request.getfixturevalue(sidecar)
    assert module.device_of(fake_torch(mps=True)) == "mps"
    assert module.device_of(fake_torch(cuda=True)) == "cuda"
    assert module.device_of(fake_torch(cuda=True, kernel_error="no kernel image")) == "cpu"
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
        (fake_torch(cuda=True, kernel_error="no kernel image"), "cpu"),
        (fake_torch(cuda=True, kernel_error="no kernel image", mps=True), "mps"),
        (fake_torch(mps=True), "mps"),
        (fake_torch(), "cpu"),
    ]:
        monkeypatch.setitem(sys.modules, "torch", torch)
        assert module.pick_device() == device
