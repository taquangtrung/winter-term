//! Reading what `docker` lists: one line per object, its fields in the order
//! the section's `--format` asks for them, separated by tabs.

// ========================================================================
// Constants
// ========================================================================

/// What docker writes for a field an object has nothing in.
const NONE: &str = "<none>";

// ========================================================================
// Data Structures
// ========================================================================

/// What the view lists: one kind of object at a time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Section {
    /// Every container, running or not.
    #[default]
    Containers,
    /// The images pulled or built.
    Images,
    /// The networks containers join.
    Networks,
    /// The volumes containers keep data in.
    Volumes,
}

/// One object docker listed: what names it to docker, what the view shows it
/// as, and the rest of its fields as the columns read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Item {
    /// The other fields, in the section's column order.
    pub columns: Vec<String>,
    /// What docker takes to name the object in a command: a container's or a
    /// network's name, an image's `repository:tag` or its id where it has no
    /// tag, a volume's name.
    pub handle: String,
    /// The object's own id, which stays put across a rename or a retag, so
    /// the cursor stays on it across a refresh.
    pub id: String,
    /// Whether a container is running, the only state the view tells apart.
    pub running: bool,
}

// ========================================================================
// Section
// ========================================================================

impl Section {
    /// Every section, in the order `Tab` steps through them.
    pub const ALL: [Section; 4] = [
        Section::Containers,
        Section::Images,
        Section::Volumes,
        Section::Networks,
    ];

    /// What the section is called, in the view's heading.
    pub fn label(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Images => "Images",
            Self::Networks => "Networks",
            Self::Volumes => "Volumes",
        }
    }

    /// The section after this one, wrapping.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The tag a listing of the section is answered under, so an answer
    /// asked for before `Tab` moved on is known for another section's.
    pub fn list_tag(self) -> &'static str {
        match self {
            Self::Containers => "docker-list-containers",
            Self::Images => "docker-list-images",
            Self::Networks => "docker-list-networks",
            Self::Volumes => "docker-list-volumes",
        }
    }

    /// The `docker` words that list the section, `--format` and all.
    pub fn list_args(self) -> Vec<String> {
        let (words, format): (&[&str], &str) = match self {
            Self::Containers => (
                &["ps", "--all"],
                "{{.ID}}\\t{{.Names}}\\t{{.Image}}\\t{{.State}}\\t{{.Status}}\\t{{.Ports}}\\t{{.Label \"com.docker.compose.project\"}}",
            ),
            Self::Images => (
                &["images"],
                "{{.ID}}\\t{{.Repository}}\\t{{.Tag}}\\t{{.Size}}\\t{{.CreatedSince}}",
            ),
            Self::Networks => (
                &["network", "ls"],
                "{{.ID}}\\t{{.Name}}\\t{{.Driver}}\\t{{.Scope}}",
            ),
            Self::Volumes => (
                &["volume", "ls"],
                "{{.Name}}\\t{{.Name}}\\t{{.Driver}}\\t{{.Scope}}",
            ),
        };
        let mut args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        args.extend(["--format".to_string(), format.to_string()]);
        args
    }

    /// The columns' headings, after the name every section leads with.
    pub fn headings(self) -> &'static [&'static str] {
        match self {
            Self::Containers => &["IMAGE", "STATE", "STATUS", "PORTS", "PROJECT"],
            Self::Images => &["SIZE", "CREATED"],
            Self::Networks => &["DRIVER", "SCOPE"],
            Self::Volumes => &["DRIVER", "SCOPE"],
        }
    }

    /// The `docker` word that names the kind of object in a command (`docker
    /// image rm`), so one command line serves every section.
    pub fn object(self) -> &'static str {
        match self {
            Self::Containers => "container",
            Self::Images => "image",
            Self::Networks => "network",
            Self::Volumes => "volume",
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The objects of `section` in `text`, what its listing printed. A line with
/// too few fields is not one docker wrote for this format, and is skipped.
pub fn parse(section: Section, text: &str) -> Vec<Item> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| item(section, line))
        .collect()
}

/// One line of `section`'s listing.
fn item(section: Section, line: &str) -> Option<Item> {
    let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
    let wanted = section.headings().len() + 2;
    if fields.len() < wanted {
        return None;
    }
    let id = fields[0].to_string();
    let (handle, rest) = match section {
        // An image is named by its repository and tag, two fields of their
        // own, or by its id where it has neither.
        Section::Images => {
            let (repository, tag) = (fields[1], fields[2]);
            let handle = match (repository, tag) {
                (NONE, _) => id.clone(),
                (_, NONE) => repository.to_string(),
                _ => format!("{repository}:{tag}"),
            };
            (handle, &fields[3..])
        }
        _ => (fields[1].to_string(), &fields[2..]),
    };
    let columns: Vec<String> = rest.iter().map(|field| field.to_string()).collect();
    let running = section == Section::Containers && columns.get(1).map(String::as_str) == Some("running");
    Some(Item {
        columns,
        handle,
        id,
        running,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_container_line_reads_into_its_columns_and_state() {
        let text = "abc123\tweb\tnginx:1\trunning\tUp 2 hours\t0.0.0.0:80->80/tcp\tshop\n\
                    def456\tdb\tpostgres\texited\tExited (0) 1 day ago\t\t\n";
        let items = parse(Section::Containers, text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].handle, "web");
        assert_eq!(items[0].columns[0], "nginx:1");
        assert!(items[0].running);
        assert!(!items[1].running);
        assert_eq!(items[1].columns[4], "", "no compose project");
    }

    #[test]
    fn test_an_image_is_named_by_its_tag_or_by_its_id() {
        let text = "ff43\tapp\tfixed\t212MB\t6 hours ago\n\
                    aa11\t<none>\t<none>\t1GB\t2 days ago\n\
                    bb22\tbase\t<none>\t5MB\t1 week ago\n";
        let handles: Vec<String> = parse(Section::Images, text)
            .into_iter()
            .map(|item| item.handle)
            .collect();
        assert_eq!(handles, ["app:fixed", "aa11", "base"]);
        assert_eq!(parse(Section::Images, text)[0].columns, ["212MB", "6 hours ago"]);
    }

    #[test]
    fn test_a_line_written_for_another_format_is_skipped() {
        assert!(parse(Section::Networks, "just one field\n").is_empty());
        assert_eq!(parse(Section::Volumes, "v\tv\tlocal\tlocal\n")[0].handle, "v");
    }

    #[test]
    fn test_a_listing_asks_docker_for_the_format_it_parses() {
        assert!(Section::Images.list_args().contains(&"--format".to_string()));
    }
}
