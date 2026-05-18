# Third-Party Notices

This project depends on third-party open-source packages. Keep each upstream
license and copyright notice when redistributing source, packaged builds,
containers, or bundles. This checklist covers direct dependencies declared in
`backend/requirements.txt` and `frontend/package.json`; release artifacts should
also include notices for transitive dependencies.

This is a practical compliance checklist, not legal advice.

## Backend

| Package | License | Attribution to retain | Source |
| --- | --- | --- | --- |
| FastAPI | MIT | Copyright (c) 2018 Sebastián Ramírez | https://github.com/fastapi/fastapi |
| Uvicorn | BSD-3-Clause | Copyright 2017-present Encode OSS Ltd | https://github.com/encode/uvicorn |
| Pydantic | MIT | Copyright 2017-present Pydantic Services Inc. and contributors | https://github.com/pydantic/pydantic |
| PyYAML | MIT | Copyright 2017-2021 Ingy döt Net; 2006-2016 Kirill Simonov | https://github.com/yaml/pyyaml |
| HTTPX | BSD-3-Clause | Copyright 2019 Encode OSS Ltd | https://github.com/encode/httpx |
| pytest | MIT | Copyright 2004 Holger Krekel and others | https://github.com/pytest-dev/pytest |

## Frontend

| Package | License | Attribution to retain | Source |
| --- | --- | --- | --- |
| @vitejs/plugin-react | MIT | Copyright 2019-present Yuxi (Evan) You and Vite contributors | https://github.com/vitejs/vite-plugin-react |
| @xyflow/react | MIT | Copyright 2019-2025 webkid GmbH | https://github.com/xyflow/xyflow |
| lucide-react | ISC | Feather portions by Cole Bemis 2013-2022; Lucide Contributors 2022 | https://lucide.dev |
| React | MIT | Copyright Facebook, Inc. and its affiliates | https://github.com/facebook/react |
| React DOM | MIT | Copyright Facebook, Inc. and its affiliates | https://github.com/facebook/react |
| TypeScript | Apache-2.0 | Apache License 2.0 notice from Microsoft/TypeScript | https://github.com/microsoft/TypeScript |
| Vite | MIT | Copyright 2019-present VoidZero Inc. and Vite contributors | https://github.com/vitejs/vite |

## Release Checklist

- Include this file with source distributions.
- Include upstream license texts when distributing bundled dependencies.
- Re-run the dependency review after adding or upgrading packages.
- Do not imply upstream projects endorse Open Pipeline Manager.
