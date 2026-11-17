use super::*;

impl Store {
    pub(super) fn validate_compat_records(&self) -> Result<(), String> {
        self.list()?;
        for key in [
            "adjustments",
            "price_overrides",
            "inventory_directory",
            "inventory_ledger",
        ] {
            let data: serde_json::Value = self.read_required(key)?;
            let array = if key == "inventory_directory" {
                "locations"
            } else {
                "events"
            };
            if !data.is_object()
                || !data.get(array).is_some_and(serde_json::Value::is_array)
                || !data
                    .get("next_id")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|id| id > 0)
            {
                return Err(format!(
                    "Saved optional workflow metadata '{key}' has an invalid shape; it was not reset."
                ));
            }
            if key == "inventory_ledger"
                && ["next_lot_id", "next_shipment_id"].iter().any(|name| {
                    !data
                        .get(*name)
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|id| id > 0)
                })
            {
                return Err(
                    "Saved inventory metadata has invalid sequence fields; it was not reset."
                        .into(),
                );
            }
        }
        if let Some(data) = self.optional_compat_record("intake_reviews")?
            && (!data.is_object() || !data.get("events").is_some_and(serde_json::Value::is_array))
        {
            return Err("Saved documentary history has an invalid shape; it was not reset.".into());
        }
        self.closed_periods()?;
        Ok(())
    }

    fn optional_compat_record(&self, key: &str) -> Result<Option<serde_json::Value>, String> {
        let mut statement = self.statement("SELECT payload FROM app_state WHERE key=?1")?;
        statement.text(1, key)?;
        if statement.step()? != ROW {
            return Ok(None);
        }
        let record: LogicalRecord<serde_json::Value> =
            serde_json::from_str(&statement.read_text(0)?)
                .map_err(|_| format!("Saved optional workflow metadata '{key}' is invalid."))?;
        if record.version != 1 {
            return Err(format!(
                "Saved optional workflow metadata '{key}' uses an unsupported record version."
            ));
        }
        Ok(Some(record.data))
    }

    pub(super) fn closed_periods(&self) -> Result<Vec<(u64, u64)>, String> {
        let Some(data) = self.optional_compat_record("closeouts")? else {
            return Ok(vec![]);
        };
        let records = data
            .as_array()
            .ok_or("Saved closed-period metadata is not an array.")?;
        records
            .iter()
            .map(|record| {
                let report = record
                    .get("report")
                    .ok_or("A saved closed period is missing its date boundaries.")?;
                let start = report
                    .get("start")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("A saved closed-period start is invalid.")?;
                let end = report
                    .get("end")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("A saved closed-period end is invalid.")?;
                if start >= end {
                    return Err("A saved closed-period interval is invalid.".into());
                }
                Ok((start, end))
            })
            .collect()
    }

    pub(super) fn validate_walkthrough_preservation(
        &self,
        previous: Option<&Ticket>,
        next: &Ticket,
    ) -> Result<(), String> {
        crate::price_overrides::validate_ordinary_candidate(previous, next)?;
        if let Some(old) = previous {
            // A legacy ordinal cannot safely identify reporting evidence after
            // line edits or removal. Contact corrections remain available.
            if old.lines != next.lines
                && old.lines.iter().any(|line| {
                    line.id == 0 && line.material_intake != crate::intake::MaterialIntake::default()
                })
            {
                return Err("This legacy draft has saved reporting details. Its material lines are read-only to preserve those details.".into());
            }
            for line in &next.lines {
                if let Some(before) = old
                    .lines
                    .iter()
                    .find(|before| line.id > 0 && before.id == line.id)
                    && before.material_intake != line.material_intake
                {
                    return Err(
                        "Stored reporting details are preserved unchanged in this build.".into(),
                    );
                }
            }
            let mut source = old.source_intake.clone();
            source.category = next.source_intake.category;
            source.reason = next.source_intake.reason;
            source.reason_detail = next.source_intake.reason_detail.clone();
            source.delivery = next.source_intake.delivery;
            source.source_name = next.source_intake.source_name.clone();
            source.source_address = next.source_intake.source_address.clone();
            source.organization = next.source_intake.organization.clone();
            source.contact_name = next.source_intake.contact_name.clone();
            source.phone = next.source_intake.phone.clone();
            source.notes = next.source_intake.notes.clone();
            source.linked_customer_id = next.source_intake.linked_customer_id;
            if source != next.source_intake {
                return Err("Stored source fields outside the counter contact form are preserved unchanged in this build.".into());
            }
        }
        Ok(())
    }
}
