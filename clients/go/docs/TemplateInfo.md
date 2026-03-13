# TemplateInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Description** | **string** | Human-readable description | 
**DetectFiles** | **[]string** | Files that indicate this runtime should be used | 
**Name** | **string** | Template name (e.g., \&quot;node20\&quot;) | 

## Methods

### NewTemplateInfo

`func NewTemplateInfo(description string, detectFiles []string, name string, ) *TemplateInfo`

NewTemplateInfo instantiates a new TemplateInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTemplateInfoWithDefaults

`func NewTemplateInfoWithDefaults() *TemplateInfo`

NewTemplateInfoWithDefaults instantiates a new TemplateInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDescription

`func (o *TemplateInfo) GetDescription() string`

GetDescription returns the Description field if non-nil, zero value otherwise.

### GetDescriptionOk

`func (o *TemplateInfo) GetDescriptionOk() (*string, bool)`

GetDescriptionOk returns a tuple with the Description field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDescription

`func (o *TemplateInfo) SetDescription(v string)`

SetDescription sets Description field to given value.


### GetDetectFiles

`func (o *TemplateInfo) GetDetectFiles() []string`

GetDetectFiles returns the DetectFiles field if non-nil, zero value otherwise.

### GetDetectFilesOk

`func (o *TemplateInfo) GetDetectFilesOk() (*[]string, bool)`

GetDetectFilesOk returns a tuple with the DetectFiles field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDetectFiles

`func (o *TemplateInfo) SetDetectFiles(v []string)`

SetDetectFiles sets DetectFiles field to given value.


### GetName

`func (o *TemplateInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *TemplateInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *TemplateInfo) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


