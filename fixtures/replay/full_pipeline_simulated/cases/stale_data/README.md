# Full Pipeline Stale Data Failure Fixture

中文说明：本 fixture 用于阶段 9 异常路径验收。组合状态时间超过风控新鲜度阈值，
运行时必须输出 `DATA_STALE` 风控拒绝、可追溯事故和只读运营报告，且不得生成执行计划。
