// 设计文档 §8.7 M5 自测: Computer Use
// - 桌面自动化：截图 + 鼠标 + 键盘 + 应用管理
// - 8 个工具：screen_screenshot/click/type/key/scroll、app_list/open/focus
// - 实现：enigo（键鼠控制）+ screenshots（截屏）+ 平台命令（应用管理）
// - 用途：测试非 Web 项目（GUI 应用、原生应用）
// - 安全：默认需用户确认每步操作（可配置白名单自动批准）

pub mod tools;

pub use tools::build_computer_use_tools;
