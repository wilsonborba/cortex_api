use crate::{
    core::config::Config,
    dal::remote::web_page::{PageReader, page_url},
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::Instant;
struct Rules {
    robot: Option<Arc<texting_robots::Robot>>,
    failed: bool,
}
/// Request-local only. An origin includes scheme and port, as robots.txt does.
#[derive(Default)]
pub struct Robots {
    rules: HashMap<String, Rules>,
    next: HashMap<String, Instant>,
}
pub enum CrawlPermission {
    Allowed(Instant),
    Denied,
    Unavailable,
    TooLate,
}
impl Robots {
    pub async fn check(
        &mut self,
        reader: &PageReader,
        config: &Config,
        target: &str,
        deadline: Instant,
    ) -> CrawlPermission {
        let Ok(url) = page_url(target) else {
            return CrawlPermission::Denied;
        };
        let origin = url.origin().ascii_serialization();
        if !self.rules.contains_key(&origin) {
            let rules = match reader.robots(target).await {
                Ok((200..=299, body)) => {
                    let agent = config.web_user_agent.clone();
                    match tokio::task::spawn_blocking(move || {
                        texting_robots::Robot::new(&agent, &body)
                    })
                    .await
                    {
                        Ok(Ok(robot)) => Rules {
                            robot: Some(Arc::new(robot)),
                            failed: false,
                        },
                        _ => Rules {
                            robot: None,
                            failed: true,
                        },
                    }
                }
                Ok((status, _)) if (400..500).contains(&status) && status != 429 => Rules {
                    robot: None,
                    failed: false,
                },
                _ => Rules {
                    robot: None,
                    failed: true,
                },
            };
            self.rules.insert(origin.clone(), rules);
        }
        let rules = &self.rules[&origin];
        if rules.failed {
            return CrawlPermission::Unavailable;
        }
        if rules
            .robot
            .as_ref()
            .is_some_and(|robot| !robot.allowed(target))
        {
            return CrawlPermission::Denied;
        }
        let now = Instant::now();
        let start = self.next.get(&origin).copied().unwrap_or(now).max(now);
        if start >= deadline {
            return CrawlPermission::TooLate;
        }
        if let Some(seconds) = rules.robot.as_ref().and_then(|r| r.delay) {
            if !seconds.is_finite() || seconds < 0.0 {
                return CrawlPermission::Unavailable;
            }
            let Ok(delay) = Duration::try_from_secs_f32(seconds) else {
                return CrawlPermission::Unavailable;
            };
            if let Some(next) = start.checked_add(delay) {
                self.next.insert(origin, next);
            } else {
                return CrawlPermission::TooLate;
            }
        }
        CrawlPermission::Allowed(start)
    }
}
