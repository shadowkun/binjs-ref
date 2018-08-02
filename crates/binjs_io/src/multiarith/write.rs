
use multiarith::{ F64, Label, Predict1, ScopeIndex, SharedTree, SubTree, Tag };

use io::TokenWriter;
use ::TokenWriterError;
use util::GenericCounter;

use std;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;

use vec_map::VecMap;

struct BitWriter; // Placeholder

#[derive(Clone, Copy, Debug)]
enum Direction {
    Enter,
    Exit
}


struct Segment {
    /// Low value for this segment.
    ///
    /// The probability of the segment is `(high - low)/context_total`.
    low:  usize,

    /// High value for this segment.
    ///
    /// MUST be greater or equal to `low`.
    high: usize,


    /// The highest possible value of `high` in this context.
    ///
    /// MUST be consistent across segments for the same context.
    ///
    /// MUST be greater or equal to `high`.
    context_highest: usize,

    /// If `true`, this is the first occurrence of this symbol in the
    /// context, so a definition must be injected.
    needs_definition: bool,
}

trait EncodingModel {
    /// Get the frequency of a tag as a child of a given parent.
    ///
    /// If the model is adaptative, this will increase the number of uses of the tag in this context by 1.
    ///
    /// `needs_definition` will always be `false` after the first call for a given tag/parent.
    fn tag_frequency_for_encoding(&mut self, tag: &Tag, parent: Option<(&Tag, usize)>) -> Result<Segment, ()>;
}

struct LinearAdaptiveEncodingModel {
    tags: Predict1<Tag, Segment>,
}


/// An encoding model which starts by analyzing the full AST to determine
/// exact statistics.
struct ExactEncodingModel {
    tags: Predict1<Tag, Segment>,
}
impl EncodingModel for ExactEncodingModel {
    fn tag_frequency_for_encoding(&mut self, tag: &Tag, parent: Option<(&Tag, usize)>) -> Result<Segment, ()> {
        let mut by_index = match parent {
            None => self.tags.by_parent.get(&None),
            Some((parent_tag, _)) => self.tags.by_parent.get(&Some(parent_tag.clone()))
        }.ok_or(())?;
        let mut by_tag = match parent {
            None => by_index.get(0),
            Some((_, index)) => by_index.get(index)
        }.ok_or(())?;
        unimplemented!()
    }
}
impl ExactEncodingModel {
    fn init_tags(predict_1: &mut Predict1<Tag, /* instances */ usize>, subtree: &SharedTree, parent: Option<(&Tag, usize)>) {
        let borrow = subtree.borrow();
        match borrow.label {
            Label::Tag(ref tag) => {
                let mut by_index = match parent {
                    None => {
                        let mut by_parent = predict_1.by_parent.entry(None)
                            .or_insert_with(|| VecMap::with_capacity(1));
                        by_parent.entry(0)
                            .or_insert_with(|| HashMap::new())
                    }
                    Some((parent, index)) => {
                        let mut by_parent = predict_1.by_parent.entry(Some(parent.clone()))
                            .or_insert_with(|| VecMap::with_capacity(5));
                        by_parent.entry(index)
                            .or_insert_with(|| HashMap::new())
                    }
                };
                let symbols = by_index.len();
                by_index.entry(tag.clone())
                    .and_modify(|instances| {
                        *instances += 1
                    })
                    .or_insert(1);

            }
            _ => {
                warn!(target: "multiarith", "Skipping initialization of predictor for label {:?} (not implemented yet)", borrow.label);
            }
        }
        // Recur towards children.
        match borrow.label {
            Label::Tag(ref tag) => {
                for (index, child) in borrow.children.iter().enumerate() {
                    Self::init_tags(predict_1, child, Some((tag, index)));
                }
            }
            _ => {
                for (index, child) in borrow.children.iter().enumerate() {
                    Self::init_tags(predict_1, child, parent);
                }
            }
        }
    }
    fn new(tree: &SharedTree) -> Self {
        let mut predict_instances_1 = Predict1::default();
        // Initialize number of instances.
        Self::init_tags(&mut predict_instances_1, tree, None);

        // Deduce probabilities.
        let probabilities = predict_instances_1.by_parent.drain()
            .map(|(parent, mut by_child_index)| {
                let by_child_index = by_child_index.drain()
                    .map(|(child_index, mut by_symbol)| {
                        let number_of_symbols = by_symbol.len();
                        let total_instances : usize = by_symbol.values()
                            .sum();
                        let mut cursor = 0;
                        let by_symbol = by_symbol.drain()
                            .enumerate()
                            .map(|(index, (tag, instances))| {
                                let low = cursor;
                                cursor += instances;
                                let high = cursor;
                                let segment = Segment {
                                    low,
                                    high,
                                    context_highest: total_instances,
                                    needs_definition: true,
                                };
                                (tag, segment)
                            })
                            .collect();
                        assert_eq!(cursor, total_instances);
                        (child_index, by_symbol)
                    })
                    .collect();
                (parent, by_child_index)
            }).collect();
        Self {
            tags: Predict1 {
                by_parent: probabilities
            }
        }
    }
}


struct PerCategory<T> {
    tags: T,
}

struct Context<'a> {
    tree: &'a SharedTree,
    parent: Option<(&'a SharedTree, usize)>,
}

struct Encoder<M> where M: EncodingModel {
    model: M,
}
impl<M> Encoder<M> where M: EncodingModel {
    fn compress(&mut self, subtree: &SharedTree, parent: Option<(&Tag, usize)>) -> Result<(), std::io::Error> {
        let borrow = subtree.borrow();
        match borrow.label {
            Label::Tag(ref tag) => {
                let segment = self.model.tag_frequency_for_encoding(tag, parent)
                    .expect("Could not compute tag frequency");
                unimplemented!("FIXME: We now have the probability of `(subtree, parent)`. Use it to refine the current segment");
                if segment.needs_definition {
                    unimplemented!("FIXME: Append definition of the current label");
                }
            }
            _ => {
                warn!(target: "multiarith", "Skipping serialization of label {:?} (not implemented yet)", borrow.label);
            }
        }
        // Recur towards children.
        match borrow.label {
            Label::Tag(ref tag) => {
                for (index, child) in borrow.children.iter().enumerate() {
                    self.compress(child, Some((tag, index)))?;
                }
            }
            _ => {
                for (index, child) in borrow.children.iter().enumerate() {
                    self.compress(child, parent)?;
                }
            }
        }
        Ok(())
    }
}

impl SubTree {
    fn with_labels<F: FnMut(&Label)>(&self, f: &mut F) {
        f(&self.label);
        for child in &self.children {
            child.borrow().with_labels(f);
        }
    }
    fn with_labels_mut<F: FnMut(Direction, &mut Label)>(&mut self, f: &mut F) {
        f(Direction::Enter, &mut self.label);
        for child in &self.children {
            child.borrow_mut().with_labels_mut(f);
        }
        f(Direction::Exit, &mut self.label);
    }
}




pub struct TreeTokenWriter {
    root: SharedTree,
    scope_counter: GenericCounter<ScopeIndex>,
}
impl TreeTokenWriter {
    pub fn new() -> Self {
        Self {
            scope_counter: GenericCounter::new(),
            root: Rc::new(RefCell::new(SubTree {
                label: Label::String(None),
                children: vec![]
            }))
        }
    }
    fn new_tree(&mut self, tree: SubTree) -> Result<SharedTree, TokenWriterError> {
        self.root = Rc::new(RefCell::new(tree));
        Ok(self.root.clone())
    }

    fn number_references(&mut self) -> Result<SharedTree, TokenWriterError> {
        // Undeclared references
        let top = Rc::new(RefCell::new(vec![]));
        let stack = Rc::new(RefCell::new(vec![top.clone()]));
        self.root.borrow_mut().with_labels_mut(&mut |direction, label| {
            let rewrite = match (direction, &label) {
                (Direction::Enter, Label::Scope(_)) => {
                    let mut borrow_stack = stack.borrow_mut();
                    borrow_stack.push(Rc::new(RefCell::new(vec![])));
                    None
                }
                (Direction::Exit, Label::Scope(_)) => {
                    let mut borrow_stack = stack.borrow_mut();
                    borrow_stack.pop();
                    None
                }
                (Direction::Enter, Label::Declare(Some(ref s))) => {
                    let borrow_stack = stack.borrow();
                    let mut borrow_frame = borrow_stack.last()
                        .unwrap()
                        .borrow_mut();
                    borrow_frame
                        .push(s.clone());
                    None
                }
                (Direction::Enter, Label::LiteralReference(None)) => {
                    Some(Label::NumberedReference(None))
                }
                (Direction::Enter, Label::LiteralReference(Some(ref s))) => {
                    let mut depth = 0;
                    let mut found = None;
                    {
                        let borrow_stack = stack.borrow();
                        'find_in_stack: for frame in borrow_stack.iter().rev() {
                            let borrow_frame = frame.borrow();
                            if let Some(index) = borrow_frame.iter()
                                .position(|name| name == s)
                            {
                                found = Some(index);
                                break 'find_in_stack;
                            } else {
                                depth += borrow_frame.len()
                            }
                        }
                    }
                    let index = match found {
                        Some(found) => found,
                        None => {
                            let mut borrow_top = top.borrow_mut();
                            borrow_top.push(s.clone());
                            borrow_top.len()
                        }
                    };
                    Some(Label::NumberedReference(Some((depth + index) as u32)))
                }
                _ => None,
            };
            if let Some(rewrite) = rewrite {
                *label = rewrite;
            }
        });

        // Now declare all undeclared variables.
        let root = self.root.clone();
        let top = top.borrow()
            .iter()
            .map(|name| Rc::new(RefCell::new(SubTree {
                label: Label::Declare(Some(name.clone())),
                children: vec![]
            })))
            .collect();
        let declared_undeclared_variables = self.new_tree(SubTree {
            label: Label::Tag(Tag::new("_undeclared_variables")),
            children: top
        })?;
        let scope_index = self.scope_counter.next();
        self.new_tree(SubTree {
            label: Label::Scope(scope_index),
            children: vec![
                declared_undeclared_variables,
                root
            ]
        })
    }
}
impl TokenWriter for TreeTokenWriter {
    type Statistics = usize; // Placeholder
    type Tree = SharedTree;
    type Data = Vec<u8>;

    fn tagged_tuple(&mut self, tag: &str, children: &[(&str, Self::Tree)]) -> Result<Self::Tree, TokenWriterError> {
        self.new_tree(SubTree {
            label: Label::Tag(Tag::new(tag)),
            children: children.iter()
                .map(|(_, tree)| tree.clone())
                .collect()
        })
    }

    fn offset(&mut self) -> Result<Self::Tree, TokenWriterError> {
        unimplemented!()
    }

    fn bool(&mut self, value: Option<bool>) -> Result<Self::Tree, TokenWriterError> {
        self.new_tree(SubTree {
            label: Label::Bool(value),
            children: vec![]
        })
    }

    fn float(&mut self, value: Option<f64>) -> Result<Self::Tree, TokenWriterError> {
        self.new_tree(SubTree {
            label: Label::Number(value.map(F64)),
            children: vec![]
        })
    }

    fn string(&mut self, value: Option<&str>) -> Result<Self::Tree, TokenWriterError> {
        self.new_tree(SubTree {
            label: Label::String(value.map(|x| Rc::new(x.to_string()))),
            children: vec![]
        })
    }

    fn string_enum(&mut self, value: &str) -> Result<Self::Tree, TokenWriterError> {
        self.tagged_tuple(value, &[])
    }

    fn list(&mut self, children: Vec<Self::Tree>) -> Result<Self::Tree, TokenWriterError> {
        self.new_tree(SubTree {
            label: Label::List(Some(children.len() as u32)),
            children
        })
    }

    fn untagged_tuple(&mut self, _: &[Self::Tree]) -> Result<Self::Tree, TokenWriterError> {
        unimplemented!()
    }

    fn done(mut self) -> Result<(Self::Data, Self::Statistics), TokenWriterError> {
        self.number_references()?;

        unimplemented!()
    }
}