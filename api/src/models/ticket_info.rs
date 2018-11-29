/*-
 * ========================LICENSE_START=================================
 * PREvant
 * %%
 * Copyright (C) 2018 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 * 
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 * 
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */
use std::convert::From;

use goji::Issue;
use serde::ser::{Serialize, Serializer};
use url::Url;

pub struct TicketInfo {
    link: Url,
    summary: String,
    status: String,
}

impl From<Issue> for TicketInfo {
    fn from(issue: Issue) -> Self {
        let summary = match issue.summary() {
            None => "".to_owned(),
            Some(summary) => summary,
        };

        let status = match issue.status() {
            None => "".to_owned(),
            Some(status) => status.name,
        };

        let mut link = Url::parse(&issue.self_link).unwrap();
        link.set_path(&("/browse/".to_owned() + &issue.key));

        TicketInfo {
            link,
            summary,
            status,
        }
    }
}

impl Serialize for TicketInfo {
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Ticket<'a> {
            link: String,
            summary: &'a String,
            status: &'a String,
        }

        let t = Ticket {
            link: self.link.to_string(),
            summary: &self.summary,
            status: &self.status,
        };

        Ok(t.serialize(serializer)?)
    }
}
