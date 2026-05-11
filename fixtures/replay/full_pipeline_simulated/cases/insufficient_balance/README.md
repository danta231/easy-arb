# Full Pipeline Insufficient Balance Failure Fixture

中文说明：本 fixture 用于阶段 9 异常路径验收。组合状态可用余额低于候选转换资本需求，
运行时必须输出 `INSUFFICIENT_BALANCE` 风控拒绝、可追溯事故和只读运营报告，且不得生成执行计划。
