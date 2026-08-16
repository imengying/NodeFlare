-- Agent 下载加速前缀（拼接在 GitHub Release 下载地址之前），空字符串表示直连。
ALTER TABLE servers ADD COLUMN agent_mirror TEXT NOT NULL DEFAULT '';
