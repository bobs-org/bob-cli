"""Safe YAML loading that preserves Python-name tags without importing them."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml
from yaml.constructor import ConstructorError
from yaml.nodes import MappingNode, ScalarNode

from .errors import DocsError

PYTHON_NAME_TAG_PREFIX = "tag:yaml.org,2002:python/name:"


@dataclass(frozen=True)
class InertPythonName:
    """A Python-name YAML tag held as data, never resolved or imported."""

    tag: str
    value: str


class _SafeLoader(yaml.SafeLoader):
    def construct_mapping(self, node: MappingNode, deep: bool = False) -> dict[Any, Any]:
        if not isinstance(node, MappingNode):
            raise ConstructorError(None, None, "expected a mapping", node.start_mark)
        mapping: dict[Any, Any] = {}
        for key_node, value_node in node.value:
            key = self.construct_object(key_node, deep=deep)
            try:
                duplicate = key in mapping
            except TypeError as error:
                raise ConstructorError(
                    "while constructing a mapping",
                    node.start_mark,
                    "found an unhashable key",
                    key_node.start_mark,
                ) from error
            if duplicate:
                raise ConstructorError(
                    "while constructing a mapping",
                    node.start_mark,
                    f"found duplicate key {key!r}",
                    key_node.start_mark,
                )
            mapping[key] = self.construct_object(value_node, deep=deep)
        return mapping


def _construct_python_name(loader: _SafeLoader, suffix: str, node: yaml.Node) -> InertPythonName:
    if not isinstance(node, ScalarNode):
        raise ConstructorError(
            "while constructing an inert Python-name tag",
            node.start_mark,
            "expected a scalar node",
            node.start_mark,
        )
    return InertPythonName(PYTHON_NAME_TAG_PREFIX + suffix, loader.construct_scalar(node))


_SafeLoader.add_multi_constructor(PYTHON_NAME_TAG_PREFIX, _construct_python_name)


class _SafeDumper(yaml.SafeDumper):
    pass


def _represent_python_name(dumper: _SafeDumper, value: InertPythonName) -> yaml.Node:
    return dumper.represent_scalar(value.tag, value.value, style="'")


_SafeDumper.add_representer(InertPythonName, _represent_python_name)


def load_yaml_text(text: str, *, source: str = "<yaml>") -> Any:
    """Load YAML using safe constructors plus inert Python-name tag preservation."""

    try:
        return yaml.load(text, Loader=_SafeLoader)
    except yaml.YAMLError as error:
        raise DocsError(f"{source}: invalid YAML: {error}") from error


def load_yaml(path: Path) -> Any:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise DocsError(f"cannot read {path}: {error}") from error
    return load_yaml_text(text, source=str(path))


def dump_yaml(value: Any) -> str:
    return yaml.dump(
        value,
        Dumper=_SafeDumper,
        allow_unicode=True,
        default_flow_style=False,
        sort_keys=False,
        width=100,
    )
